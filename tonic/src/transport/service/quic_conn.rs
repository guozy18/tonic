use core::pin::Pin;
use futures_core::{
    task::{Context, Poll},
    Future,
};
// use h3_quinn::{SendStream, RecvStream};
use http::Uri;
use hyper::client::connect::Connected as HyperConnected;
use hyper::client::connect::Connection as HyperConnection;
use quinn::{self, ClientConfig, Endpoint};
use quinn::{RecvStream, SendStream};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net,
};
use tower::Service;
// use pin_project::pin_project;

pub(crate) struct QuicConnector {
    config: Option<ClientConfig>,
}

pub(crate) struct QuicConnecting {
    fut: Pin<Box<dyn Future<Output = Result<QuicStream, crate::Error>> + Send>>,
}

pub(crate) struct QuicStream {
    send_stream: SendStream,
    recv_stream: RecvStream,
}

impl QuicConnector {
    /// Get a new `QuicConnector` instance. If `config` is `None` then use
    /// the default client configuration.
    pub(crate) fn new(config: Option<ClientConfig>) -> Self {
        Self { config }
    }
}

impl Service<Uri> for QuicConnector {
    type Response = QuicStream;
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
            let conn = new_conn.connection;

            let (send_stream, recv_stream) = conn.open_bi().await.map_err(Box::new)?;
            Ok(QuicStream {
                send_stream,
                recv_stream,
            })
        });

        QuicConnecting { fut }
    }
}

impl Future for QuicConnecting {
    type Output = Result<QuicStream, crate::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = Pin::new(&mut self.fut);
        fut.poll(cx)
    }
}

impl AsyncRead for QuicStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let recv_stream = Pin::new(&mut self.recv_stream);
        recv_stream.poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let send_stream = Pin::new(&mut self.send_stream);
        send_stream.poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let send_stream = Pin::new(&mut self.send_stream);
        send_stream.poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let send_stream = Pin::new(&mut self.send_stream);
        send_stream.poll_shutdown(cx)
    }
}

impl HyperConnection for QuicStream {
    fn connected(&self) -> HyperConnected {
        HyperConnected::new().proxy(true)
    }
}
