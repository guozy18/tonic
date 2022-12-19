use std::error::Error as StdError;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::body::BoxBody;
use crate::transport::service::ServerIo;
use futures::Future;
use h3_quinn::NewConnection;
use http::{Request, Response};
use http_body::Body as HttpBody;
use tower::Service;
use tracing::debug;

use pin_project_lite::pin_project;

use h3_quinn::quinn::{Connecting, Incoming};

use super::Connected;

macro_rules! ready {
    ($e:expr) => {
        match $e {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

pin_project! {
    /// A listening Quic server that accepts quic connections by default.
    ///
    /// `Server` is a `Future` mapping a bound listener with a set of service
    /// handlers. It is built using the [`Builder`](Builder), and the future
    /// completes when the server has been shutdown.
    pub struct QuicServer <S, IO> {
        #[pin]
        incoming: Incoming,
        make_service: S,
        #[pin]
        state: State,
        _io: PhantomData<fn() -> IO>,
    }
}

pin_project! {
    #[project = StateProj]
    pub enum State {
        Idle,
        Connecting {
            #[pin]
            connecting: Connecting,
        },
        Connected {
            // future: Future,
            // #[pin]
            connection: NewConnection,
        },
    }
}

impl<S, IO> QuicServer<S, IO> {
    /// Starts a [`Builder`](Builder) with the provided incoming stream.
    pub fn builder(incoming: Incoming) -> QuicBuilder {
        QuicBuilder { incoming }
    }

    /// Binds to the provided address, and returns a [`Builder`](Builder).
    ///
    /// # Panics
    ///
    /// This method will panic if binding to the address fails. For a method
    /// to bind to an address and return a `Result`, see `Server::try_bind`.
    pub fn bind(addr: &SocketAddr) -> QuicBuilder {
        // let incoming = AddrIncoming::new(addr).unwrap_or_else(|e| {
        //     panic!("error binding to {}: {}", addr, e);
        // });
        unimplemented!()
    }

    /// Tries to bind to the provided address, and returns a [`Builder`](Builder).
    pub fn try_bind(addr: &SocketAddr) -> crate::Result<QuicBuilder> {
        // AddrIncoming::new(addr).map(Server::builder)
        unimplemented!()
    }
}

impl<S, IO> QuicServer<S, IO>
where
    // S: Service<ServerIo<IO>, Response = Response<BoxBody>> + Clone + Send + 'static,
    S: Service<ServerIo<IO>> + Clone + Send + 'static,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
    // S: Service<Request<Body>, Response = Response<ResBody>> + Clone + Send + 'static,
    // S::Future: Send + 'static,
    // S::Error: Into<crate::Error> + Send,
    // ResBody: http_body::Body<Data = Bytes> + Send + 'static,
    // ResBody::Error: Into<crate::Error>,
    IO: Connected,
{
    fn poll_next_(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        service_fut: Pin<&mut impl Future>,
    ) -> Poll<crate::Result<()>> {
        let mut me = self.project();

        loop {
            let next = {
                match me.state.as_mut().project() {
                    StateProj::Idle => {
                        unreachable!()
                    }
                    StateProj::Connecting { connecting } => {
                        // construct make service and QuicServer but not run
                        let res = ready!(connecting.poll(cx));
                        // IO Input

                        let conn = match res {
                            Ok(conn) => conn,
                            Err(err) => {
                                let err = crate::Error::from(err);
                                debug!("connecting error: {}", err);
                                return Poll::Ready(Ok(()));
                            }
                        };
                        State::Connected { connection: conn }
                    }
                    // Step3. run server, get request and return response
                    StateProj::Connected { connection } => {
                        // construct inner server.
                        // me.make_service.call(req)
                        let _service = ready!(service_fut.poll(cx));
                        // ready!(inner_server)
                        let new_fut = async move {
                            debug!("New connection now established");
                            // let mut h3_conn =
                            //     h3::server::Connection::new(h3_quinn::Connection::new(*connection))
                            //         .await
                            //         .unwrap();
                            // loop {
                            //     match h3_conn.accept().await {
                            //         Ok(Some((req, stream))) => {
                            //             // let root = ;
                            //             debug!("New request: {:#?}", req);

                            //             tokio::spawn(async {
                            //                 // 在这里构建QUICServer的请求
                            //                 // !FIXME! todo!()
                            //                 let status = StatusCode::OK;
                            //                 let resp = http::Response::builder()
                            //                     .status(status)
                            //                     .body(())
                            //                     .unwrap();

                            //                 match stream.send_response(resp).await {
                            //                     Ok(_) => {
                            //                         debug!("Response to connection successful");
                            //                     }
                            //                     Err(err) => {
                            //                         error!("Unable to send response to connection peer: {:?}", err);
                            //                     }
                            //                 }

                            //                 // let buf = service.call(req).await;
                            //                 // stream.send_data(buf.freeze()).await;
                            //                 // if let Err(e) = stream.finish().await {
                            //                 //     error!("request failed: {}", e);
                            //                 // }
                            //             });
                            //         }
                            //         Ok(None) => {
                            //             break;
                            //         }
                            //         Err(_) => todo!(),
                            //     }
                            // }
                        };

                        futures::pin_mut!(new_fut);
                        let x = ready!(new_fut.poll(cx));
                        return Poll::Ready(Ok(x));
                    }
                }
            };

            me.state.set(next);
        }
    }

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        todo!()
        // let mut me = self.project();

        // loop {
        //     match ready!(me.make_service.poll_ready(cx)) {
        //         Ok(()) => (),
        //         Err(e) => {
        //             return Poll::Ready(Err(crate::Status::from_error_generic(e)));
        //         }
        //     }

        //     if let Some(connecting) = ready!(me.incoming.poll_next(cx)) {
        //         let new_fut = me.make_service.call();
        //         futures::pin_mut!(new_fut);
        //         let new_state = State::Connecting { connecting };
        //         // me.state.set(new_state);
        //         self.poll_next_(cx, new_fut);
        //     } else {
        //         return Poll::Ready(Ok(()));
        //     }
        // }
    }
}

impl<S, IO> Future for QuicServer<S, IO>
where
    S: Service<ServerIo<IO>, Response = Response<BoxBody>> + Clone + Send + 'static,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
    IO: Connected,
{
    type Output = crate::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll_next(cx)
    }
}

pub struct QuicBuilder {
    incoming: Incoming,
}

// ===== impl Builder =====

impl QuicBuilder {
    /// Start a new builder, wrapping an incoming stream and low-level options.
    ///
    /// For a more convenient constructor, see [`Server::bind`](Server::bind).
    pub fn new(incoming: Incoming) -> Self {
        QuicBuilder { incoming }
    }

    /// Consume this `QuicBuilder`, creating a [`QuicServer`](QuicServer).
    ///
    pub fn serve<S, B, IO>(self, make_service: S) -> QuicServer<S, IO>
    where
        S: Service<Request<BoxBody>, Response = Response<BoxBody>> + Clone + Send + 'static,
        S::Error: Into<Box<dyn StdError + Send + Sync>>,
        B: HttpBody + 'static,
        B::Error: Into<Box<dyn StdError + Send + Sync>>,
        IO: Connected,
    {
        QuicServer {
            incoming: self.incoming,
            make_service,
            state: State::Idle,
            _io: PhantomData,
        }
    }
}

// #[tokio::main]
// async fn main() -> Result<(), Box<dyn std::error::Error>> {
//     let root: Option<PathBuf> = None;
//     let root = if let Some(root) = root {
//         if !root.is_dir() {
//             return Err(format!("{}: is not a readable directory", root.display()).into());
//         } else {
//             info!("serving {}", root.display());
//             Arc::new(Some(root))
//         }
//     } else {
//         Arc::new(None)
//     };

//     let crypto = load_crypto().await?;
//     let server_config = h3_quinn::quinn::ServerConfig::with_crypto(Arc::new(crypto));
//     let listen: SocketAddr = "127.0.0.1:4433".parse().unwrap();
//     let (endpoint, mut incoming) = h3_quinn::quinn::Endpoint::server(server_config, listen)?;

//     info!("Listening on {}", listen);

//     while let Some(new_conn) = incoming.next().await {
//         trace_span!("New connection being attempted");

//         let root = root.clone();
//         tokio::spawn(async move {
//             match new_conn.await {
//                 Ok(conn) => {
//                     debug!("New connection now established");

//                     let mut h3_conn = h3::server::Connection::new(h3_quinn::Connection::new(conn))
//                         .await
//                         .unwrap();
//                     loop {
//                         match h3_conn.accept().await {
//                             Ok(Some((req, stream))) => {
//                                 let root = root.clone();
//                                 debug!("New request: {:#?}", req);

//                                 tokio::spawn(async {
//                                     if let Err(e) = handle_request(req, stream, root).await {
//                                         error!("request failed: {}", e);
//                                     }
//                                 });
//                             }
//                             Ok(None) => {
//                                 break;
//                             }
//                             Err(err) => {
//                                 warn!("error on accept {}", err);
//                                 match err.get_error_level() {
//                                     ErrorLevel::ConnectionError => break,
//                                     ErrorLevel::StreamError => continue,
//                                 }
//                             }
//                         }
//                     }
//                 }
//                 Err(err) => {
//                     warn!("accepting connection failed: {:?}", err);
//                 }
//             }
//         });
//     }

//     endpoint.wait_idle().await;

//     Ok(())
// }

// async fn handle_request<T>(
//     req: Request<()>,
//     mut stream: RequestStream<T, Bytes>,
//     serve_root: Arc<Option<PathBuf>>,
// ) -> Result<(), Box<dyn std::error::Error>>
// where
//     T: BidiStream<Bytes>,
// {
//     let (status, to_serve) = match serve_root.as_deref() {
//         None => (StatusCode::OK, None),
//         Some(_) if req.uri().path().contains("..") => (StatusCode::NOT_FOUND, None),
//         Some(root) => {
//             let to_serve = root.join(req.uri().path().strip_prefix('/').unwrap_or(""));
//             match File::open(&to_serve).await {
//                 Ok(file) => (StatusCode::OK, Some(file)),
//                 Err(e) => {
//                     debug!("failed to open: \"{}\": {}", to_serve.to_string_lossy(), e);
//                     (StatusCode::NOT_FOUND, None)
//                 }
//             }
//         }
//     };

//     let resp = http::Response::builder().status(status).body(()).unwrap();

//     match stream.send_response(resp).await {
//         Ok(_) => {
//             debug!("Response to connection successful");
//         }
//         Err(err) => {
//             error!("Unable to send response to connection peer: {:?}", err);
//         }
//     }

//     if let Some(mut file) = to_serve {
//         loop {
//             let mut buf = BytesMut::with_capacity(4096 * 10);
//             if file.read_buf(&mut buf).await? == 0 {
//                 break;
//             }
//             stream.send_data(buf.freeze()).await?;
//         }
//     }

//     Ok(stream.finish().await?)
// }

// static ALPN: &[u8] = b"h3";

// async fn load_crypto() -> Result<rustls::ServerConfig, Box<dyn std::error::Error>> {
//     let cert: Option<PathBuf> = None;
//     let key: Option<PathBuf> = None;
//     let (cert, key) = match (cert, key) {
//         (None, None) => build_certs(),
//         (Some(cert_path), Some(ref key_path)) => {
//             let mut cert_v = Vec::new();
//             let mut key_v = Vec::new();

//             let mut cert_f = File::open(cert_path).await?;
//             let mut key_f = File::open(key_path).await?;

//             cert_f.read_to_end(&mut cert_v).await?;
//             key_f.read_to_end(&mut key_v).await?;
//             (rustls::Certificate(cert_v), PrivateKey(key_v))
//         }
//         (_, _) => return Err("cert and key args are mutually dependant".into()),
//     };

//     let mut crypto = rustls::ServerConfig::builder()
//         .with_safe_default_cipher_suites()
//         .with_safe_default_kx_groups()
//         .with_protocol_versions(&[&rustls::version::TLS13])
//         .unwrap()
//         .with_no_client_auth()
//         .with_single_cert(vec![cert], key)?;
//     crypto.max_early_data_size = u32::MAX;
//     crypto.alpn_protocols = vec![ALPN.into()];

//     Ok(crypto)
// }

// pub fn build_certs() -> (Certificate, PrivateKey) {
//     let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
//     let key = PrivateKey(cert.serialize_private_key_der());
//     let cert = Certificate(cert.serialize_der().unwrap());
//     (cert, key)
// }
