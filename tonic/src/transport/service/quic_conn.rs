use core::pin::Pin;
use futures_core::{
    task::{Context, Poll},
    Future,
};
// use h3_quinn::{SendStream, RecvStream};
use http::Uri;
use hyper::client::connect::Connected as HyperConnected;
use hyper::client::connect::Connection as HyperConnection;
use h3_quinn::{Endpoint, Connection as H3Connection};
use quinn::{self, ClientConfig};
use quinn::{RecvStream, SendStream};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net,
};
use tower::Service;

pub(crate) struct QuicConnector {
    config: Option<ClientConfig>,
}

pub(crate) struct QuicConnecting {
    fut: Pin<Box<dyn Future<Output = Result<h3_quinn::Connection, crate::Error>> + Send>>,
}

impl QuicConnector {
    /// Get a new `QuicConnector` instance. If `config` is `None` then use
    /// the default client configuration.
    pub(crate) fn new(config: Option<ClientConfig>) -> Self {
        Self { config }
    }
}

impl Service<Uri> for QuicConnector {
    type Response = h3_quinn::Connection;
    type Error = crate::Error;
    type Future = QuicConnecting;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let mut ep = Endpoint::client("[::]:0".parse().unwrap()).unwrap();
        ep.set_default_client_config(
            self.config
                .take()
                .unwrap_or_else(|| ClientConfig::with_native_roots()),
        );

        let fut = Box::pin(async move {
            let name = req.host().unwrap_or("Unknown");
            let auth = req.authority().unwrap();
            let port = auth.port_u16().unwrap_or(443);

            let addr = net::lookup_host((auth.host(), port))
                .await
                .map_err(Box::new)?
                .next()
                .unwrap();
            let new_conn = ep
                .connect(addr, name)
                .map_err(Box::new)?
                .await
                .map_err(Box::new)?;
            Ok(H3Connection::new(new_conn))
        });

        QuicConnecting { fut }
    }
}

impl Future for QuicConnecting {
    type Output = Result<h3_quinn::Connection, crate::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = Pin::new(&mut self.fut);
        fut.poll(cx)
    }
}
