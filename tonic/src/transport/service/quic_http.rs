use bytes::Bytes;
use futures_core::Future;
use h3::client::{Connection as H3Connection, SendRequest as H3SendRequest};
use h3::quic::{Connection as QuicConnection, OpenStreams};
use http::{Request, Response};
use http_body::Body as HttpBody;
use hyper::Body;
use std::marker::PhantomData;
use std::pin::Pin;
use tower::make::MakeConnection;
use tower::Service;

pub(crate) struct Http3Connector<M, O, C> {
    inner: M,
    _marker: PhantomData<fn(O, C)>,
}

impl<M, O, C> Http3Connector<M, O, C> {
    pub(crate) fn new(inner: M) -> Self {
        Self {
            inner,
            _marker: PhantomData,
        }
    }
}

pub(crate) struct SendRequest<O, C>
where
    C: QuicConnection<Bytes>,
    O: OpenStreams<Bytes>,
{
    conn: H3Connection<C, Bytes>,
    sr: H3SendRequest<O, Bytes>,
}

impl<M, T, O, C> Service<T> for Http3Connector<M, O, C>
where
    M: MakeConnection<T> + Send,
    M::Connection: Unpin + Send + 'static,
    M::Future: Send + 'static,
    M::Error: Into<crate::Error> + Send,
    T: Send,
    C: QuicConnection<Bytes> + Send,
    O: OpenStreams<Bytes> + Send,
{
    type Response = SendRequest<O, C>;
    type Error = crate::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: T) -> Self::Future {
        let make_conn = self.inner.make_connection(req);
        Box::pin(async move {
            let conn = make_conn.await.map_err(Into::into)?;
            // let (conn, sr) = h3::client::new(conn).await.map_err(Into::into)?;
            // Ok(SendRequest { conn, sr })
            todo!()
        })
    }
}

impl<C, O, HB> Service<Request<HB>> for SendRequest<O, C>
where
    HB: HttpBody + 'static,
    C: QuicConnection<Bytes>,
    O: OpenStreams<Bytes>,
{
    type Response = Response<Body>;
    type Error = crate::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        todo!()
    }

    fn call(&mut self, req: Request<HB>) -> Self::Future {
        todo!()
    }
}
