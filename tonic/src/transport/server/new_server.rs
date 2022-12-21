use super::{MakeSvc, Routes};
pub use crate::server::NamedService;

use super::conn::Connected;
use crate::body::BoxBody;
use bytes::Bytes;
use futures_core::Stream;
use http::{Request, Response};
use hyper::Body;
use rustls::{Certificate, PrivateKey};
use std::{
    convert::Infallible, fmt, future::Future, marker::PhantomData, net::SocketAddr, path::PathBuf,
    sync::Arc, time::Duration,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
};
use tower::{
    layer::util::{Identity, Stack},
    layer::Layer,
    Service, ServiceBuilder,
};
// new import
use super::QuicServer;

type BoxHttpBody = http_body::combinators::UnsyncBoxBody<Bytes, crate::Error>;
type BoxService =
    tower::util::BoxService<Request<BoxHttpBody>, Response<BoxHttpBody>, crate::Error>;
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
pub struct NewServer<L = Identity> {
    trace_interceptor: Option<TraceInterceptor>,
    concurrency_limit: Option<usize>,
    timeout: Option<Duration>,
    service_builder: ServiceBuilder<L>,
}

/// A stack based `Service` router.
#[derive(Debug)]
pub struct NewRouter<L = Identity> {
    server: NewServer<L>,
    routes: Routes,
}

impl<L> NewRouter<L> {
    pub(crate) fn new(server: NewServer<L>, routes: Routes) -> Self {
        Self { server, routes }
    }
}

impl Default for NewServer<Identity> {
    fn default() -> Self {
        Self {
            trace_interceptor: None,
            concurrency_limit: None,
            timeout: None,
            service_builder: Default::default(),
        }
    }
}

impl NewServer {
    /// Create a new server builder that can configure a [`NewServer`].
    pub fn builder() -> Self {
        NewServer {
            ..Default::default()
        }
    }
}

impl<L> NewServer<L> {
    /// Set the concurrency limit applied to on requests inbound per connection.
    #[must_use]
    pub fn concurrency_limit_per_connection(self, limit: usize) -> Self {
        NewServer {
            concurrency_limit: Some(limit),
            ..self
        }
    }

    /// Set a timeout on for all request handlers.
    #[must_use]
    pub fn timeout(self, timeout: Duration) -> Self {
        NewServer {
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
        NewServer {
            trace_interceptor: Some(Arc::new(f)),
            ..self
        }
    }

    /// Create a router with the `S` typed service as the first service.
    ///
    /// This will clone the `NewServer` builder and create a router that will
    /// route around different services.
    pub fn add_service<S>(&mut self, svc: S) -> NewRouter<L>
    where
        S: Service<Request<Body>, Response = Response<BoxBody>, Error = Infallible>
            + NamedService
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
        L: Clone,
    {
        NewRouter::new(self.clone(), Routes::new(svc))
    }

    /// Create a router with the optional `S` typed service as the first service.
    ///
    /// This will clone the `NewServer` builder and create a router that will
    /// route around different services.
    ///
    /// # Note
    /// Even when the argument given is `None` this will capture *all* requests to this service name.
    /// As a result, one cannot use this to toggle between two identically named implementations.
    pub fn add_optional_service<S>(&mut self, svc: Option<S>) -> NewRouter<L>
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
        NewRouter::new(self.clone(), routes)
    }

    /// Set the [Tower] [`Layer`] all services will be wrapped in.
    ///
    /// This enables using middleware from the [Tower ecosystem][eco].
    pub fn layer<NewLayer>(self, new_layer: NewLayer) -> NewServer<Stack<NewLayer, L>> {
        NewServer {
            service_builder: self.service_builder.layer(new_layer),
            trace_interceptor: self.trace_interceptor,
            concurrency_limit: self.concurrency_limit,
            timeout: self.timeout,
        }
    }

    // current the signal only support None
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
        let listen: SocketAddr = "127.0.0.1:4433".parse().unwrap();
        let trace_interceptor = self.trace_interceptor.clone();
        let concurrency_limit = self.concurrency_limit;
        let timeout = self.timeout;

        let svc = self.service_builder.service(svc);

        let crypto = load_crypto().await.unwrap();
        let server_config = h3_quinn::quinn::ServerConfig::with_crypto(Arc::new(crypto));
        let (endpoint, mut incoming) =
            h3_quinn::quinn::Endpoint::server(server_config, listen).unwrap();

        let svc: MakeSvc<<L as Layer<S>>::Service, IO> = MakeSvc {
            inner: svc,
            concurrency_limit,
            timeout,
            trace_interceptor,
            _io: PhantomData,
        };

        // let incoming = accept::from_stream::<_, _, crate::Error>(tcp);
        // let server = hyper::Server::builder(incoming);
        let server = QuicServer::builder(incoming);

        if let Some(signal) = signal {
            server.serve(svc).await.map_err(super::Error::from_source)?
        } else {
            server.serve(svc).await.map_err(super::Error::from_source)?;
        }

        Ok(())
    }
}

impl<L> fmt::Debug for NewServer<L> {
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

// impl<E, S, IB, OB, IO> MakeServiceRef<IB> for MakeSvc<S, IO>
// where
//     E: Into<Box<dyn StdError + Send + Sync>>,
//     S: HttpService<IB, ResBody = OB, Error = E>,
//     IB: HttpBody,
//     OB: HttpBody,
//     IO: Connected,
// {
//     type Error = E;
//     type Service = S;
//     type ResBody = OB;
//     type MakeError = crate::Error;
//     type Future = dyn Future<Output = crate::Result<S>>;

//     type __DontNameMe = super::quicserver::sealed::CantName;

//     fn poll_ready_ref(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>> {
//         self.poll_ready(cx)
//     }

//     fn make_service_ref(&mut self, target: &) -> Self::Future {
//         self.call(target)
//     }
// }

// impl<S, B1, B2, IO> super::quicserver::sealed::Sealed<B1> for MakeSvc<S, IO>
// where
//     // T: for<'a> Service<&'a Target, Response = S>,
//     IO: Connected,
//     S: HttpService<B1, ResBody = B2>,
//     B1: HttpBody,
//     B2: HttpBody,
// {
// }

// struct Svc<S> {
//     inner: S,
//     trace_interceptor: Option<TraceInterceptor>,
// }

// impl<S, ResBody> Service<Request<Body>> for Svc<S>
// where
//     S: Service<Request<Body>, Response = Response<ResBody>>,
//     S::Error: Into<crate::Error>,
//     ResBody: http_body::Body<Data = Bytes> + Send + 'static,
//     ResBody::Error: Into<crate::Error>,
// {
//     type Response = Response<BoxHttpBody>;
//     type Error = crate::Error;
//     type Future = SvcFuture<S::Future>;

//     fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         self.inner.poll_ready(cx).map_err(Into::into)
//     }

//     fn call(&mut self, mut req: Request<Body>) -> Self::Future {
//         let span = if let Some(trace_interceptor) = &self.trace_interceptor {
//             let (parts, body) = req.into_parts();
//             let bodyless_request = Request::from_parts(parts, ());

//             let span = trace_interceptor(&bodyless_request);

//             let (parts, _) = bodyless_request.into_parts();
//             req = Request::from_parts(parts, body);

//             span
//         } else {
//             tracing::Span::none()
//         };

//         SvcFuture {
//             inner: self.inner.call(req),
//             span,
//         }
//     }
// }

// #[pin_project]
// struct SvcFuture<F> {
//     #[pin]
//     inner: F,
//     span: tracing::Span,
// }

// impl<F, E, ResBody> Future for SvcFuture<F>
// where
//     F: Future<Output = Result<Response<ResBody>, E>>,
//     E: Into<crate::Error>,
//     ResBody: http_body::Body<Data = Bytes> + Send + 'static,
//     ResBody::Error: Into<crate::Error>,
// {
//     type Output = Result<Response<BoxHttpBody>, crate::Error>;

//     fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let this = self.project();
//         let _guard = this.span.enter();

//         let response: Response<ResBody> = ready!(this.inner.poll(cx)).map_err(Into::into)?;
//         let response = response.map(|body| body.map_err(Into::into).boxed_unsync());
//         Poll::Ready(Ok(response))
//     }
// }

// impl<S> fmt::Debug for Svc<S> {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         f.debug_struct("Svc").finish()
//     }
// }

// #[derive(Clone)]
// struct MakeSvc<S, IO> {
//     concurrency_limit: Option<usize>,
//     timeout: Option<Duration>,
//     inner: S,
//     trace_interceptor: Option<TraceInterceptor>,
//     _io: PhantomData<fn() -> IO>,
// }

// impl<S, ResBody, IO> Service<ServerIo<IO>> for MakeSvc<S, IO>
// where
//     IO: Connected,
//     S: Service<Request<Body>, Response = Response<ResBody>> + Clone + Send + 'static,
//     S::Future: Send + 'static,
//     S::Error: Into<crate::Error> + Send,
//     ResBody: http_body::Body<Data = Bytes> + Send + 'static,
//     ResBody::Error: Into<crate::Error>,
// {
//     type Response = BoxService;
//     type Error = crate::Error;
//     type Future = future::Ready<Result<Self::Response, Self::Error>>;

//     fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         Ok(()).into()
//     }

//     fn call(&mut self, io: ServerIo<IO>) -> Self::Future {
//         let conn_info = io.connect_info();

//         let svc = self.inner.clone();
//         let concurrency_limit = self.concurrency_limit;
//         let timeout = self.timeout;
//         let trace_interceptor = self.trace_interceptor.clone();

//         let svc = ServiceBuilder::new()
//             .layer_fn(RecoverError::new)
//             .option_layer(concurrency_limit.map(ConcurrencyLimitLayer::new))
//             .layer_fn(|s| GrpcTimeout::new(s, timeout))
//             .service(svc);

//         let svc = ServiceBuilder::new()
//             .layer(BoxService::layer())
//             .map_request(move |mut request: Request<Body>| {
//                 match &conn_info {
//                     tower::util::Either::A(inner) => {
//                         request.extensions_mut().insert(inner.clone());
//                     }
//                     tower::util::Either::B(inner) => {
//                         #[cfg(feature = "tls")]
//                         {
//                             request.extensions_mut().insert(inner.clone());
//                             request.extensions_mut().insert(inner.get_ref().clone());
//                         }

//                         #[cfg(not(feature = "tls"))]
//                         {
//                             // just a type check to make sure we didn't forget to
//                             // insert this into the extensions
//                             let _: &() = inner;
//                         }
//                     }
//                 }

//                 request
//             })
//             .service(Svc {
//                 inner: svc,
//                 trace_interceptor,
//             });

//         future::ready(Ok(svc))
//     }
// }
