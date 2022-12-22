use bytes::Bytes;
use futures_core::Future;
use h3::quic::Connection as QuicConnection;
use http::{Request, Response};
use http_body::combinators::UnsyncBoxBody;
use http_body::Body as HttpBody;
use hyper::Body;
use hyper::rt::Executor;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tower::make::MakeConnection;
use tower::Service;

use crate::Status;

use super::executor;

pub(crate) struct Http3Connector<M, C> {
    inner: M,
    _marker: PhantomData<C>,
}

impl<M, C> Http3Connector<M, C> {
    pub(crate) fn new(inner: M) -> Self {
        Self {
            inner,
            _marker: PhantomData,
        }
    }
}

pub(crate) struct SendRequest<T>(mpsc::UnboundedSender<T>);

impl<M, T, C> Service<T> for Http3Connector<M, C>
where
    M: MakeConnection<T, Connection = C> + Send,
    M::Connection: Unpin + Send + 'static,
    M::Future: Send + 'static,
    M::Error: Into<crate::Error> + Send,
    T: Send,
    C: QuicConnection<Bytes> + Send,
{
    type Response = SendRequest<Request<UnsyncBoxBody<Bytes, Status>>>;
    type Error = crate::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: T) -> Self::Future {
        let make_conn = self.inner.make_connection(req);
        Box::pin(async move {
            let conn = make_conn.await.map_err(Into::into)?;
            let (_conn, sr) = h3::client::new(conn).await.map_err(Box::new)?;
            let (tx, mut rx) = mpsc::unbounded_channel();
            executor::SharedExec::tokio().execute(Box::pin(async move {
                if let Some(data) = rx.recv().await {
                    // TODO send request
                }
            }));
            Ok(SendRequest(tx))
        })
    }
}

impl<T> Clone for SendRequest<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<HB> Service<Request<HB>> for SendRequest<Request<HB>>
where
    HB: HttpBody + 'static + Send + Debug,
{
    type Response = Response<Body>;
    type Error = crate::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<HB>) -> Self::Future {
        self.0.send(req).unwrap();
        Box::pin(async move { todo!() })
    }
}
