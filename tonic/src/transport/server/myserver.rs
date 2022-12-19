use super::Routes;
pub use crate::server::NamedService;

use super::conn::Connected;
use crate::body::BoxBody;
use bytes::Bytes;
use futures_core::Stream;
use http::{Request, Response};
use hyper::Body;
use rustls::{Certificate, PrivateKey};
use std::{convert::Infallible, fmt, future::Future, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
};
use tower::{
    layer::util::{Identity, Stack},
    layer::Layer,
    Service, ServiceBuilder,
};

type BoxHttpBody = http_body::combinators::UnsyncBoxBody<Bytes, crate::Error>;
type BoxService = tower::util::BoxService<Request<Body>, Response<BoxHttpBody>, crate::Error>;
type TraceInterceptor = Arc<dyn Fn(&http::Request<()>) -> tracing::Span + Send + Sync + 'static>;

const DEFAULT_HTTP2_KEEPALIVE_TIMEOUT_SECS: u64 = 20;

/// A default batteries included `transport` server.
///
/// This is a wrapper around [`hyper::Server`] and provides an easy builder
/// pattern style builder [`Server`]. This builder exposes easy configuration parameters
/// for providing a fully featured http2 based gRPC server. This should provide
/// a very good out of the box http2 server for use with tonic but is also a
/// reference implementation that should be a good starting point for anyone
/// wanting to create a more complex and/or specific implementation.
#[derive(Clone)]
pub struct MyServer<L = Identity> {
    trace_interceptor: Option<TraceInterceptor>,
    concurrency_limit: Option<usize>,
    timeout: Option<Duration>,
    service_builder: ServiceBuilder<L>,
}

/// A stack based `Service` router.
#[derive(Debug)]
pub struct QuicRouter<L = Identity> {
    server: MyServer<L>,
    routes: Routes,
}

impl<L> QuicRouter<L> {
    pub(crate) fn new(server: MyServer<L>, routes: Routes) -> Self {
        Self { server, routes }
    }
}

impl Default for MyServer<Identity> {
    fn default() -> Self {
        Self {
            trace_interceptor: None,
            concurrency_limit: None,
            timeout: None,
            service_builder: Default::default(),
        }
    }
}

impl MyServer {
    /// Create a new server builder that can configure a [`MyServer`].
    pub fn builder() -> Self {
        MyServer {
            ..Default::default()
        }
    }
}

impl<L> MyServer<L> {
    /// Set the concurrency limit applied to on requests inbound per connection.
    ///
    /// # Example
    ///
    /// ```
    /// # use tonic::transport::MyServer;
    /// # use tower_service::Service;
    /// # let builder = MyServer::builder();
    /// builder.concurrency_limit_per_connection(32);
    /// ```
    #[must_use]
    pub fn concurrency_limit_per_connection(self, limit: usize) -> Self {
        MyServer {
            concurrency_limit: Some(limit),
            ..self
        }
    }

    /// Set a timeout on for all request handlers.
    ///
    /// # Example
    ///
    /// ```
    /// # use tonic::transport::MyServer;
    /// # use tower_service::Service;
    /// # use std::time::Duration;
    /// # let builder = MyServer::builder();
    /// builder.timeout(Duration::from_secs(30));
    /// ```
    #[must_use]
    pub fn timeout(self, timeout: Duration) -> Self {
        MyServer {
            timeout: Some(timeout),
            ..self
        }
    }

    /// Intercept inbound headers and add a [`tracing::Span`] to each response future.
    #[must_use]
    pub fn trace_fn<F>(self, f: F) -> Self
    where
        F: Fn(&http::Request<()>) -> tracing::Span + Send + Sync + 'static,
    {
        MyServer {
            trace_interceptor: Some(Arc::new(f)),
            ..self
        }
    }

    /// Create a router with the `S` typed service as the first service.
    ///
    /// This will clone the `MyServer` builder and create a router that will
    /// route around different services.
    pub fn add_service<S>(&mut self, svc: S) -> QuicRouter<L>
    where
        S: Service<Request<Body>, Response = Response<BoxBody>, Error = Infallible>
            + NamedService
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
        L: Clone,
    {
        QuicRouter::new(self.clone(), Routes::new(svc))
    }

    /// Create a router with the optional `S` typed service as the first service.
    ///
    /// This will clone the `MyServer` builder and create a router that will
    /// route around different services.
    ///
    /// # Note
    /// Even when the argument given is `None` this will capture *all* requests to this service name.
    /// As a result, one cannot use this to toggle between two identically named implementations.
    pub fn add_optional_service<S>(&mut self, svc: Option<S>) -> QuicRouter<L>
    where
        S: Service<Request<Body>, Response = Response<BoxBody>, Error = Infallible>
            + NamedService
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
        L: Clone,
    {
        let routes = svc.map(Routes::new).unwrap_or_default();
        QuicRouter::new(self.clone(), routes)
    }

    /// Set the [Tower] [`Layer`] all services will be wrapped in.
    ///
    /// This enables using middleware from the [Tower ecosystem][eco].
    ///
    /// # Example
    ///
    /// ```
    /// # use tonic::transport::MyServer;
    /// # use tower_service::Service;
    /// use tower::timeout::TimeoutLayer;
    /// use std::time::Duration;
    ///
    /// # let mut builder = MyServer::builder();
    /// builder.layer(TimeoutLayer::new(Duration::from_secs(30)));
    /// ```
    ///
    /// Note that timeouts should be set using [`MyServer::timeout`]. `TimeoutLayer` is only used
    /// here as an example.
    ///
    /// You can build more complex layers using [`ServiceBuilder`]. Those layers can include
    /// [interceptors]:
    ///
    /// ```
    /// # use tonic::transport::MyServer;
    /// # use tower_service::Service;
    /// use tower::ServiceBuilder;
    /// use std::time::Duration;
    /// use tonic::{Request, Status, service::interceptor};
    ///
    /// fn auth_interceptor(request: Request<()>) -> Result<Request<()>, Status> {
    ///     if valid_credentials(&request) {
    ///         Ok(request)
    ///     } else {
    ///         Err(Status::unauthenticated("invalid credentials"))
    ///     }
    /// }
    ///
    /// fn valid_credentials(request: &Request<()>) -> bool {
    ///     // ...
    ///     # true
    /// }
    ///
    /// fn some_other_interceptor(request: Request<()>) -> Result<Request<()>, Status> {
    ///     Ok(request)
    /// }
    ///
    /// let layer = ServiceBuilder::new()
    ///     .load_shed()
    ///     .timeout(Duration::from_secs(30))
    ///     .layer(interceptor(auth_interceptor))
    ///     .layer(interceptor(some_other_interceptor))
    ///     .into_inner();
    ///
    /// MyServer::builder().layer(layer);
    /// ```
    ///
    /// [Tower]: https://github.com/tower-rs/tower
    /// [`Layer`]: tower::layer::Layer
    /// [eco]: https://github.com/tower-rs
    /// [`ServiceBuilder`]: tower::ServiceBuilder
    /// [interceptors]: crate::service::Interceptor
    pub fn layer<NewLayer>(self, new_layer: NewLayer) -> MyServer<Stack<NewLayer, L>> {
        MyServer {
            service_builder: self.service_builder.layer(new_layer),
            trace_interceptor: self.trace_interceptor,
            concurrency_limit: self.concurrency_limit,
            timeout: self.timeout,
        }
    }

    pub(crate) async fn serve_with_shutdown<S, I, F, IO, IE, ResBody>(
        self,
        svc: S,
        incoming: I,
        signal: Option<F>,
    ) -> Result<(), super::Error>
    where
        L: Layer<S>,
        L::Service: Service<Request<Body>, Response = Response<ResBody>> + Clone + Send + 'static,
        <<L as Layer<S>>::Service as Service<Request<Body>>>::Future: Send + 'static,
        <<L as Layer<S>>::Service as Service<Request<Body>>>::Error: Into<crate::Error> + Send,
        I: Stream<Item = Result<IO, IE>>,
        IO: AsyncRead + AsyncWrite + Connected + Unpin + Send + 'static,
        IO::ConnectInfo: Clone + Send + Sync + 'static,
        IE: Into<crate::Error>,
        F: Future<Output = ()>,
        ResBody: http_body::Body<Data = Bytes> + Send + 'static,
        ResBody::Error: Into<crate::Error>,
    {
        // my tmp define
        // let listen: SocketAddr = "127.0.0.1:4433".parse().unwrap();
        // let trace_interceptor = self.trace_interceptor.clone();
        // let concurrency_limit = self.concurrency_limit;
        // let timeout = self.timeout;

        // let svc = self.service_builder.service(svc);

        // let crypto = load_crypto().await.unwrap();
        // let server_config = h3_quinn::quinn::ServerConfig::with_crypto(Arc::new(crypto));
        // let (endpoint, mut incoming) =
        //     h3_quinn::quinn::Endpoint::server(server_config, listen).unwrap();

        // let svc = MakeSvc {
        //     inner: svc,
        //     concurrency_limit,
        //     timeout,
        //     trace_interceptor,
        //     _io: PhantomData,
        // };

        // let incoming = accept::from_stream::<_, _, crate::Error>(tcp);
        // let server = hyper::Server::builder(incoming);

        // if let Some(signal) = signal {
        //     server
        //         .serve(svc)
        //         .with_graceful_shutdown(signal)
        //         .await
        //         .map_err(super::Error::from_source)?
        // } else {
        //     server.serve(svc).await.map_err(super::Error::from_source)?;
        // }

        Ok(())
    }
}

impl<L> fmt::Debug for MyServer<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder").finish()
    }
}

static ALPN: &[u8] = b"h3";

async fn load_crypto() -> Result<rustls::ServerConfig, Box<dyn std::error::Error>> {
    let cert: Option<PathBuf> = None;
    let key: Option<PathBuf> = None;
    let (cert, key) = match (cert, key) {
        (None, None) => build_certs(),
        (Some(cert_path), Some(ref key_path)) => {
            let mut cert_v = Vec::new();
            let mut key_v = Vec::new();

            let mut cert_f = File::open(cert_path).await?;
            let mut key_f = File::open(key_path).await?;

            cert_f.read_to_end(&mut cert_v).await?;
            key_f.read_to_end(&mut key_v).await?;
            (rustls::Certificate(cert_v), PrivateKey(key_v))
        }
        (_, _) => return Err("cert and key args are mutually dependant".into()),
    };

    let mut crypto = rustls::ServerConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    crypto.max_early_data_size = u32::MAX;
    crypto.alpn_protocols = vec![ALPN.into()];

    Ok(crypto)
}

pub fn build_certs() -> (Certificate, PrivateKey) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let key = PrivateKey(cert.serialize_private_key_der());
    let cert = Certificate(cert.serialize_der().unwrap());
    (cert, key)
}
