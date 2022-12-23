use std::error::Error as StdError;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::service::HttpService;
use futures::Future;
use h3_quinn::NewConnection;
use http::{Request, StatusCode};
use http_body::Body as HttpBody;
use hyper::Body;
// use hyper::server::conn::Http;
use bytes::{Buf, Bytes, BytesMut};
use tower::Service;
use tracing::{debug, error};

use futures_util::stream::Stream;
use pin_project_lite::pin_project;

use h3_quinn::quinn::{Connecting, Incoming};
// use super::{Connected, BoxService};

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
    pub struct QuicServer <S> {
        #[pin]
        incoming: Incoming,
        make_service: S,
        #[pin]
        state: State,
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
            connection: NewConnection,
        },
    }
}

impl QuicServer<()> {
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
    pub fn bind(_addr: &SocketAddr) -> QuicBuilder {
        // let incoming = AddrIncoming::new(addr).unwrap_or_else(|e| {
        //     panic!("error binding to {}: {}", addr, e);
        // });
        unimplemented!()
    }

    /// Tries to bind to the provided address, and returns a [`Builder`](Builder).
    pub fn try_bind(_addr: &SocketAddr) -> crate::Result<QuicBuilder> {
        // AddrIncoming::new(addr).map(Server::builder)
        unimplemented!()
    }
}

impl<S, B> QuicServer<S>
where
    S: MakeServiceRef<(), Body, ResBody = B>,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
    B: HttpBody + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
    // F: Future<Output = Result<S::Service, S::MakeError>,
    // IO: Connected,
{
    // S: HttpService<IB, ResBody = OB, Error = E>,
    // F: Future<Output = Result<S, ME>>,
    // IB: HttpBody,
    // OB: HttpBody,
    fn poll_next_(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        service_fut: Pin<&mut dyn Future<Output = Result<S::Service, S::MakeError>>>,
        // service_fut: dyn Future<Output = Result<S::Service, S::MakeError>>,
    ) -> Poll<crate::Result<()>> {
        let mut me = self.project();

        loop {
            match me.state.as_mut().project() {
                StateProj::Idle => {
                    unreachable!()
                }
                StateProj::Connecting { connecting } => {
                    // Step1. Get Quic NewConnection
                    let res = ready!(connecting.poll(cx));
                    let conn = match res {
                        Ok(conn) => conn,
                        Err(err) => {
                            let err = crate::Error::from(err);
                            debug!("connecting error: {}", err);
                            return Poll::Ready(Ok(()));
                        }
                    };

                    // Step2. construct inner server.
                    let service = ready!(service_fut.poll(cx));
                    let mut inner_service: S::Service = match service {
                        Ok(service) => service,
                        Err(err) => {
                            let err = err.into();
                            debug!("connecting error: {}", err);
                            return Poll::Ready(Ok(()));
                        }
                    };

                    let server_fut = async move {
                        debug!("New connection now established");
                        let mut h3_conn: h3::server::Connection<h3_quinn::Connection, Bytes> =
                            h3::server::Connection::new(h3_quinn::Connection::new(conn))
                                .await
                                .unwrap();
                        // let mut h3_conn: h3::server::Connection<h3_quinn::Connection, Bytes> =
                        //     h3::server::Connection::new(h3_quinn::Connection::new(conn))
                        //         .await
                        //         .unwrap();
                        loop {
                            match h3_conn.accept().await {
                                Ok(Some((req, mut stream))) => {
                                    debug!("New request: {:#?}", req);
                                    let body = Body::empty();
                                    let (parts, _) = req.into_parts();
                                    let req = Request::from_parts(parts, body);

                                    let resp = inner_service.call(req).await;

                                    let resp = match resp {
                                        Ok(resp) => resp,
                                        Err(err) => {
                                            return;
                                        }
                                    };

                                    // 在这里构建QUICServer的请求
                                    // !FIXME! todo!()
                                    let (parts, body) = resp.into_parts();
                                    let resp = http::Response::from_parts(parts, ());

                                    match stream.send_response(resp).await {
                                        Ok(_) => {
                                            debug!("Response to connection successful");
                                        }
                                        Err(err) => {
                                            error!(
                                                "Unable to send response to connection peer: {:?}",
                                                err
                                            );
                                        }
                                    }

                                    // let mut buf = BytesMut::with_capacity(4096 * 10);
                                    // // buf
                                    // stream.send_data(buf.freeze()).await;
                                }
                                Ok(None) => {
                                    break;
                                }
                                Err(_) => todo!(),
                            }
                        }
                    };

                    futures::pin_mut!(server_fut);
                    ready!(server_fut.poll(cx));
                    return Poll::Ready(Ok(()));
                }
                StateProj::Connected { connection } => unreachable!(),
            }
        }
    }

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        loop {
            let mut me = self.as_mut().project();

            // Step1. whether the make_service_service is ready
            match ready!(me.make_service.poll_ready_ref(cx)) {
                Ok(()) => (),
                Err(e) => {
                    return Poll::Ready(Err(crate::Status::from_error_generic(e)));
                }
            }

            // if the inner service is ready, then poll_next to get the incoming connecting.
            if let Some(connecting) = ready!(me.incoming.as_mut().poll_next(cx)) {
                let new_fut = me.make_service.make_service_ref(());
                futures::pin_mut!(new_fut);
                let new_state = State::Connecting { connecting };
                me.state.set(new_state);

                let _x: crate::Result<()> = ready!(self.as_mut().poll_next_(cx, new_fut));
            } else {
                return Poll::Ready(Ok(()));
            }
        }
    }
}

impl<S, B> Future for QuicServer<S>
where
    S: MakeServiceRef<(), Body, ResBody = B>,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
    B: HttpBody + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
    // F: Future<Output = Result<S::Service, S::MakeError>,
{
    type Output = crate::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll_next(cx)
    }
}

#[derive(Debug)]
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
    pub fn serve<S, B>(self, make_service: S) -> QuicServer<S>
    where
        S: MakeServiceRef<(), Body, ResBody = B>,
        S::Error: Into<Box<dyn StdError + Send + Sync>>,
        B: HttpBody + 'static,
        B::Error: Into<Box<dyn StdError + Send + Sync>>,
    {
        QuicServer {
            incoming: self.incoming,
            make_service,
            state: State::Idle,
            // _io: PhantomData,
        }
    }
}

// Just a sort-of "trait alias" of `MakeService`, not to be implemented
// by anyone, only used as bounds.
pub trait MakeServiceRef<Target, ReqBody>: sealed::Sealed<(Target, ReqBody)> {
    type ResBody: HttpBody;
    type Error: Into<Box<dyn StdError + Send + Sync>>;
    type Service: HttpService<ReqBody, ResBody = Self::ResBody, Error = Self::Error>;
    type MakeError: Into<Box<dyn StdError + Send + Sync>>;
    type Future: Future<Output = Result<Self::Service, Self::MakeError>>;

    // Acting like a #[non_exhaustive] for associated types of this trait.
    //
    // Basically, no one outside of hyper should be able to set this type
    // or declare bounds on it, so it should prevent people from creating
    // trait objects or otherwise writing code that requires using *all*
    // of the associated types.
    //
    // Why? So we can add new associated types to this alias in the future,
    // if necessary.
    type __DontNameMe: sealed::CantImpl;

    fn poll_ready_ref(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>>;

    fn make_service_ref(&mut self, target: Target) -> Self::Future;
}
// pub trait MakeServiceRef<Target, ReqBody>: sealed::Sealed<(Target, ReqBody)> {
//     type ResBody: HttpBody;
//     type Error: Into<Box<dyn StdError + Send + Sync>>;
//     type Service: HttpService<ReqBody, ResBody = Self::ResBody, Error = Self::Error>;
//     type MakeError: Into<Box<dyn StdError + Send + Sync>>;
//     type Future: Future<Output = Result<Self::Service, Self::MakeError>>;

//     // Acting like a #[non_exhaustive] for associated types of this trait.
//     //
//     // Basically, no one outside of hyper should be able to set this type
//     // or declare bounds on it, so it should prevent people from creating
//     // trait objects or otherwise writing code that requires using *all*
//     // of the associated types.
//     //
//     // Why? So we can add new associated types to this alias in the future,
//     // if necessary.
//     type __DontNameMe: sealed::CantImpl;

//     fn poll_ready_ref(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>>;

//     fn make_service_ref(&mut self, target: Target) -> Self::Future;
// }

impl<T, Target, E, ME, S, F, IB, OB> MakeServiceRef<Target, IB> for T
where
    T: Service<Target, Error = ME, Response = S, Future = F>,
    E: Into<Box<dyn StdError + Send + Sync>>,
    ME: Into<Box<dyn StdError + Send + Sync>>,
    S: HttpService<IB, ResBody = OB, Error = E>,
    F: Future<Output = Result<S, ME>>,
    IB: HttpBody,
    OB: HttpBody,
{
    type Error = E;
    type Service = S;
    type ResBody = OB;
    type MakeError = ME;
    type Future = F;

    type __DontNameMe = sealed::CantName;

    fn poll_ready_ref(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>> {
        self.poll_ready(cx)
    }

    fn make_service_ref(&mut self, target: Target) -> Self::Future {
        self.call(target)
    }
}

impl<T, Target, S, B1, B2> sealed::Sealed<(Target, B1)> for T
where
    T: Service<Target, Response = S>,
    S: HttpService<B1, ResBody = B2>,
    B1: HttpBody,
    B2: HttpBody,
{
}

mod sealed {
    pub trait Sealed<X> {}

    #[allow(unreachable_pub)] // This is intentional.
    pub trait CantImpl {}

    #[allow(missing_debug_implementations)]
    pub enum CantName {}

    impl CantImpl for CantName {}
}

pin_project! {
    /// A future building a new `Service` to a `Connection`.
    ///
    /// Wraps the future returned from `MakeService` into one that returns
    /// a `Connection`.
    #[derive(Debug)]
    pub struct InnerService<F> {
        #[pin]
        future: F,
    }
}

impl<F, S, FE, B> Future for InnerService<F>
where
    F: Future<Output = Result<S, FE>>,
    S: HttpService<Body, ResBody = B>,
    B: HttpBody + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    type Output = Result<S, FE>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = self.project();
        let service = ready!(me.future.poll(cx))?;
        Poll::Ready(Ok(service))
    }
}
