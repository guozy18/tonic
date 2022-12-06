use futures_core::{
    ready,
    task::{Context, Poll},
    Future,
};
use h3_quinn::{quinn::Endpoint, Connection, ConnectionError};
use http::Uri;
use tower::Service;
// use futures_util::future::poll_fn;
use core::pin::Pin;
use quinn::ClientConfig;
use std::net::SocketAddr;

struct QuicConnector {
    config: ClientConfig,
}

struct QuicConnecting {
    ep: Endpoint,
    addr: SocketAddr,
}

impl Service<Uri> for QuicConnector {
    type Response = Connection;
    type Error = ConnectionError;
    type Future = QuicConnecting;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Uri) -> Self::Future {
        QuicConnecting::new()
    }
}

impl QuicConnecting {
    fn new() -> Self {
        todo!()
    }
}

impl Future for QuicConnecting {
    type Output = Result<Connection, ConnectionError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = self
            .ep
            .connect(self.addr, "xxx")
            .map_err(|_| quinn::ConnectionError::LocallyClosed)
            .map_err(ConnectionError::from); // XXX gg
        if let Err(e) = result {
            return Poll::Ready(Err(e));
        }
        let mut connecting = result.unwrap();
        let conn = ready!(Pin::new(&mut connecting).poll(cx))?;
        Poll::Ready(Ok(Connection::new(conn)))
    }
}
