use bytes::BytesMut;
use futures_core::Future;
use futures_util::stream::poll_fn;
use http::request::Parts;
use http::{Uri, Response};
use tower::Service;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use http_body::Body as HttpBody;

use super::quic_conn::QuicStream;

pub(crate) struct Http3Connector<C> {
    inner: C
}

pub(crate) struct SendRequest {
    stream: QuicStream,
}

impl<C> Http3Connector<C> {
    pub(crate) fn new(inner: C) -> Self {
        Self { inner }
    }
}

impl<C, T> Service<T> for Http3Connector<C>
where
    C: Service<Uri> + Send + 'static,
    C::Error: Into<crate::Error> + Send,
    C::Future: Unpin + Send,
    C::Response: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Response = SendRequest;
    type Error = crate::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: T) -> Self::Future {
        todo!()
    }
}

impl<B> Service<http::Request<B>> for SendRequest
where
    B: HttpBody + 'static,
{
    type Response = Response<hyper::Body>;
    type Error = crate::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        todo!()
    }
}