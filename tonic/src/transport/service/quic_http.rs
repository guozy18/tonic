use futures_core::{ Future};
use h3::client::SendRequest;
use http::{Response, Uri};
use http_body::Body as HttpBody;
use std::pin::Pin;
use tower::make::MakeConnection;
use tower::Service;


pub(crate) struct Http3Connector<C> {
    inner: C,
}

impl<C> Http3Connector<C> {
    pub(crate) fn new(inner: C) -> Self {
        Self { inner }
    }
}

impl<M, T> Service<T> for Http3Connector<M>
where
    M: MakeConnection<T> + Send,
    M::Connection: Unpin + Send,
    M::Future: Send + 'static,
    M::Error: Into<crate::Error> + Send,
    T: Send,
{
    type Response = SendRequest;
    type Error = crate::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

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
            todo!()
        })
    }
}
