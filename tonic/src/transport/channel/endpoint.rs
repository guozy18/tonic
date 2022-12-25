use super::super::service;
use super::Channel;
#[cfg(feature = "tls")]
use super::ClientTlsConfig;
#[cfg(feature = "tls")]
use crate::transport::service::TlsConnector;
use crate::transport::{
    service::{QuicConnector, SharedExec},
    Error, Executor,
};
use bytes::Bytes;
use http::{uri::Uri, HeaderValue};
use quinn::{ClientConfig, TransportConfig};
use std::{
    convert::{TryFrom, TryInto},
    fmt,
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tower::make::MakeConnection;

/// Channel builder.
///
/// This struct is used to build and configure HTTP/3 channels.
#[derive(Clone)]
pub struct Endpoint {
    pub(crate) uri: Uri,
    pub(crate) origin: Option<Uri>,
    pub(crate) user_agent: Option<HeaderValue>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) concurrency_limit: Option<usize>,
    pub(crate) rate_limit: Option<(u64, Duration)>,
    #[cfg(feature = "tls")]
    pub(crate) tls: Option<TlsConnector>,
    pub(crate) buffer_size: Option<usize>,
    pub(crate) quic_version: Option<u32>,
    pub(crate) quic_max_concurrent_bidi_streams: Option<u32>,
    pub(crate) quic_max_concurrent_uni_streams: Option<u32>,
    pub(crate) quic_stream_receive_window: Option<u32>,
    pub(crate) quic_receive_window: Option<u32>,
    pub(crate) quic_send_window: Option<u32>,
    pub(crate) quic_keep_alive_interval: Option<Duration>,
    #[allow(dead_code)] // add timeout connector later
    pub(crate) connect_timeout: Option<Duration>,
    pub(crate) executor: SharedExec,
}

impl Endpoint {
    #[doc(hidden)]
    pub fn new<D>(dst: D) -> Result<Self, Error>
    where
        D: TryInto<Self>,
        D::Error: Into<crate::Error>,
    {
        let me = dst.try_into().map_err(|e| Error::from_source(e.into()))?;
        Ok(me)
    }

    /// Convert an `Endpoint` from a static string.
    ///
    /// # Panics
    ///
    /// This function panics if the argument is an invalid URI.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// Endpoint::from_static("https://example.com");
    /// ```
    pub fn from_static(s: &'static str) -> Self {
        let uri = Uri::from_static(s);
        Self::from(uri)
    }

    /// Convert an `Endpoint` from shared bytes.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// Endpoint::from_shared("https://example.com".to_string());
    /// ```
    pub fn from_shared(s: impl Into<Bytes>) -> Result<Self, Error> {
        let uri = Uri::from_maybe_shared(s.into()).map_err(|e| Error::new_invalid_uri().with(e))?;
        Ok(Self::from(uri))
    }

    /// Set a custom user-agent header.
    ///
    /// `user_agent` will be prepended to Tonic's default user-agent string (`tonic/x.x.x`).
    /// It must be a value that can be converted into a valid  `http::HeaderValue` or building
    /// the endpoint will fail.
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # let mut builder = Endpoint::from_static("https://example.com");
    /// builder.user_agent("Greeter").expect("Greeter should be a valid header value");
    /// // user-agent: "Greeter tonic/x.x.x"
    /// ```
    pub fn user_agent<T>(self, user_agent: T) -> Result<Self, Error>
    where
        T: TryInto<HeaderValue>,
    {
        user_agent
            .try_into()
            .map(|ua| Endpoint {
                user_agent: Some(ua),
                ..self
            })
            .map_err(|_| Error::new_invalid_user_agent())
    }

    /// Set a custom origin.
    ///
    /// Override the `origin`, mainly useful when you are reaching a Server/LoadBalancer
    /// which serves multiple services at the same time.
    /// It will play the role of SNI (Server Name Indication).
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # let mut builder = Endpoint::from_static("https://proxy.com");
    /// builder.origin("https://example.com".parse().expect("http://example.com must be a valid URI"));
    /// // origin: "https://example.com"
    /// ```
    pub fn origin(self, origin: Uri) -> Self {
        Endpoint {
            origin: Some(origin),
            ..self
        }
    }

    /// Apply a timeout to each request.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # use std::time::Duration;
    /// # let mut builder = Endpoint::from_static("https://example.com");
    /// builder.timeout(Duration::from_secs(5));
    /// ```
    ///
    /// # Notes
    ///
    /// This does **not** set the timeout metadata (`grpc-timeout` header) on
    /// the request, meaning the server will not be informed of this timeout,
    /// for that use [`Request::set_timeout`].
    ///
    /// [`Request::set_timeout`]: crate::Request::set_timeout
    pub fn timeout(self, dur: Duration) -> Self {
        Endpoint {
            timeout: Some(dur),
            ..self
        }
    }

    /// Apply a timeout to connecting to the uri.
    ///
    /// Defaults to no timeout.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # use std::time::Duration;
    /// # let mut builder = Endpoint::from_static("https://example.com");
    /// builder.connect_timeout(Duration::from_secs(5));
    /// ```
    pub fn connect_timeout(self, dur: Duration) -> Self {
        Endpoint {
            connect_timeout: Some(dur),
            ..self
        }
    }

    /// Set the QUIC version to use.
    pub fn quic_version(self, ver: u32) -> Self {
        Endpoint {
            quic_version: Some(ver),
            ..self
        }
    }

    /// Maximum number of incoming bidirectional streams that may be open concurrently.
    ///
    /// Must be nonzero for the peer to open any bidirectional streams.
    ///
    /// Worst-case memory use is directly proportional to max_concurrent_bidi_streams *
    /// stream_receive_window, with an upper bound proportional to receive_window.
    pub fn quic_max_concurrent_bidi_streams(self, limit: u32) -> Self {
        Endpoint {
            quic_max_concurrent_bidi_streams: Some(limit),
            ..self
        }
    }

    /// Variant of max_concurrent_bidi_streams affecting unidirectional streams.
    pub fn quic_max_concurrent_uni_streams(self, limit: u32) -> Self {
        Endpoint {
            quic_max_concurrent_uni_streams: Some(limit),
            ..self
        }
    }

    /// Maximum number of bytes the peer may transmit without acknowledgement on any one
    /// stream before becoming blocked.
    ///
    /// This should be set to at least the expected connection latency multiplied by the
    /// maximum desired throughput. Setting this smaller than receive_window helps ensure
    /// that a single stream doesn’t monopolize receive buffers, which may otherwise occur
    /// if the application chooses not to read from a large stream for a time while still
    /// requiring data on other streams.
    pub fn quic_stream_receive_window(self, winsz: u32) -> Self {
        Endpoint {
            quic_stream_receive_window: Some(winsz),
            ..self
        }
    }

    /// Maximum number of bytes the peer may transmit across all streams of a connection
    /// before becoming blocked.
    ///
    /// This should be set to at least the expected connection latency multiplied by the
    /// maximum desired throughput. Larger values can be useful to allow maximum throughput
    /// within a stream while another is blocked.
    pub fn quic_receive_window(self, winsz: u32) -> Self {
        Endpoint {
            quic_receive_window: Some(winsz),
            ..self
        }
    }

    /// Maximum number of bytes to transmit to a peer without acknowledgment.
    ///
    /// Provides an upper bound on memory when communicating with peers that issue large
    /// amounts of flow control credit. Endpoints that wish to handle large numbers of
    /// connections robustly should take care to set this low enough to guarantee memory
    /// exhaustion does not occur if every connection uses the entire window.
    pub fn quic_send_window(self, winsz: u32) -> Self {
        Endpoint {
            quic_send_window: Some(winsz),
            ..self
        }
    }

    /// Period of inactivity before sending a keep-alive packet.
    ///
    /// Keep-alive packets prevent an inactive but otherwise healthy connection from timing
    /// out.
    ///
    /// None to disable, which is the default. Only one side of any given connection needs
    /// keep-alive enabled for the connection to be preserved. Must be set lower than the
    /// idle_timeout of both peers to be effective.
    pub fn quic_keep_alive_interval(self, duration: Duration) -> Self {
        Endpoint {
            quic_keep_alive_interval: Some(duration),
            ..self
        }
    }

    /// Apply a concurrency limit to each request.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # let mut builder = Endpoint::from_static("https://example.com");
    /// builder.concurrency_limit(256);
    /// ```
    pub fn concurrency_limit(self, limit: usize) -> Self {
        Endpoint {
            concurrency_limit: Some(limit),
            ..self
        }
    }

    /// Apply a rate limit to each request.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # use std::time::Duration;
    /// # let mut builder = Endpoint::from_static("https://example.com");
    /// builder.rate_limit(32, Duration::from_secs(1));
    /// ```
    pub fn rate_limit(self, limit: u64, duration: Duration) -> Self {
        Endpoint {
            rate_limit: Some((limit, duration)),
            ..self
        }
    }

    /// Configures TLS for the endpoint.
    #[cfg(feature = "tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls")))]
    pub fn tls_config(self, tls_config: ClientTlsConfig) -> Result<Self, Error> {
        Ok(Endpoint {
            tls: Some(
                tls_config
                    .tls_connector(self.uri.clone())
                    .map_err(Error::from_source)?,
            ),
            ..self
        })
    }

    /// Sets the executor used to spawn async tasks.
    ///
    /// Uses `tokio::spawn` by default.
    pub fn executor<E>(mut self, executor: E) -> Self
    where
        E: Executor<Pin<Box<dyn Future<Output = ()> + Send>>> + Send + Sync + 'static,
    {
        self.executor = SharedExec::new(executor);
        self
    }

    /// Create a channel from this config.
    pub async fn connect(&self) -> Result<Channel, Error> {
        let mut config = ClientConfig::with_native_roots();
        if let Some(version) = self.quic_version {
            config.version(version);
        }

        let mut transport_config = TransportConfig::default();
        transport_config.keep_alive_interval(self.quic_keep_alive_interval);
        if let Some(value) = self.quic_max_concurrent_bidi_streams {
            transport_config.max_concurrent_bidi_streams(value.into());
        }
        if let Some(value) = self.quic_max_concurrent_uni_streams {
            transport_config.max_concurrent_uni_streams(value.into());
        }
        if let Some(value) = self.quic_stream_receive_window {
            transport_config.stream_receive_window(value.into());
        }
        if let Some(value) = self.quic_receive_window {
            transport_config.receive_window(value.into());
        }
        if let Some(value) = self.quic_send_window {
            transport_config.send_window(value.into());
        }

        config.transport = Arc::new(transport_config);
        let quic = QuicConnector::new(Some(config));

        #[cfg(feature = "tls")]
        let connector = service::connector(quic, self.tls.clone());

        #[cfg(not(feature = "tls"))]
        let connector = service::connector(http);

        // if let Some(connect_timeout) = self.connect_timeout {
        //     let mut connector = hyper_timeout::TimeoutConnector::new(connector);
        //     connector.set_connect_timeout(Some(connect_timeout));
        //     Channel::connect(connector, self.clone()).await
        // } else {
        //     Channel::connect(connector, self.clone()).await
        // }

        Channel::connect(connector, self.clone()).await
    }

    /// Create a channel from this config.
    ///
    /// The channel returned by this method does not attempt to connect to the endpoint until first
    /// use.
    pub fn connect_lazy(&self) -> Channel {
        let mut config = ClientConfig::with_native_roots();
        if let Some(version) = self.quic_version {
            config.version(version);
        }

        let mut transport_config = TransportConfig::default();
        transport_config.keep_alive_interval(self.quic_keep_alive_interval);
        if let Some(value) = self.quic_max_concurrent_bidi_streams {
            transport_config.max_concurrent_bidi_streams(value.into());
        }
        if let Some(value) = self.quic_max_concurrent_uni_streams {
            transport_config.max_concurrent_uni_streams(value.into());
        }
        if let Some(value) = self.quic_stream_receive_window {
            transport_config.stream_receive_window(value.into());
        }
        if let Some(value) = self.quic_receive_window {
            transport_config.receive_window(value.into());
        }
        if let Some(value) = self.quic_send_window {
            transport_config.send_window(value.into());
        }

        config.transport = Arc::new(transport_config);
        let quic = QuicConnector::new(Some(config));

        #[cfg(feature = "tls")]
        let connector = service::connector(quic, self.tls.clone());

        #[cfg(not(feature = "tls"))]
        let connector = service::connector(quic);

        // if let Some(connect_timeout) = self.connect_timeout {
        //     let mut connector = hyper_timeout::TimeoutConnector::new(connector);
        //     connector.set_connect_timeout(Some(connect_timeout));
        //     Channel::new(connector, self.clone())
        // } else {
        //     Channel::new(connector, self.clone())
        // }

        Channel::new(connector, self.clone())
    }

    /// Connect with a custom connector.
    ///
    /// This allows you to build a [Channel](struct.Channel.html) that uses a non-HTTP transport.
    /// See the `uds` example for an example on how to use this function to build channel that
    /// uses a Unix socket transport.
    ///
    /// The [`connect_timeout`](Endpoint::connect_timeout) will still be applied.
    pub async fn connect_with_connector<C>(&self, connector: C) -> Result<Channel, Error>
    where
        C: MakeConnection<Uri> + Send + 'static,
        C::Connection: Unpin + Send + 'static,
        C::Future: Send + 'static,
        crate::Error: From<C::Error> + Send + 'static,
    {
        #[cfg(feature = "tls")]
        let connector = service::connector(connector, self.tls.clone());

        #[cfg(not(feature = "tls"))]
        let connector = service::connector(connector);

        // if let Some(connect_timeout) = self.connect_timeout {
        //     let mut connector = hyper_timeout::TimeoutConnector::new(connector);
        //     connector.set_connect_timeout(Some(connect_timeout));
        //     Channel::connect(connector, self.clone()).await
        // } else {
        //     Channel::connect(connector, self.clone()).await
        // }

        Channel::connect(connector, self.clone()).await
    }

    /// Connect with a custom connector lazily.
    ///
    /// This allows you to build a [Channel](struct.Channel.html) that uses a non-HTTP transport
    /// connect to it lazily.
    ///
    /// See the `uds` example for an example on how to use this function to build channel that
    /// uses a Unix socket transport.
    pub fn connect_with_connector_lazy<C>(&self, connector: C) -> Channel
    where
        C: MakeConnection<Uri> + Send + 'static,
        C::Connection: Unpin + Send + 'static,
        C::Future: Send + 'static,
        crate::Error: From<C::Error> + Send + 'static,
    {
        #[cfg(feature = "tls")]
        let connector = service::connector(connector, self.tls.clone());

        #[cfg(not(feature = "tls"))]
        let connector = service::connector(connector);

        Channel::new(connector, self.clone())
    }

    /// Get the endpoint uri.
    ///
    /// ```
    /// # use tonic::transport::Endpoint;
    /// # use http::Uri;
    /// let endpoint = Endpoint::from_static("https://example.com");
    ///
    /// assert_eq!(endpoint.uri(), &Uri::from_static("https://example.com"));
    /// ```
    pub fn uri(&self) -> &Uri {
        &self.uri
    }
}

impl From<Uri> for Endpoint {
    fn from(uri: Uri) -> Self {
        Self {
            uri,
            origin: None,
            user_agent: None,
            concurrency_limit: None,
            rate_limit: None,
            timeout: None,
            #[cfg(feature = "tls")]
            tls: None,
            buffer_size: None,
            quic_version: None,
            quic_max_concurrent_bidi_streams: None,
            quic_max_concurrent_uni_streams: None,
            quic_stream_receive_window: None,
            quic_receive_window: None,
            quic_send_window: None,
            quic_keep_alive_interval: None,
            connect_timeout: None,
            executor: SharedExec::tokio(),
        }
    }
}

impl TryFrom<Bytes> for Endpoint {
    type Error = Error;

    fn try_from(t: Bytes) -> Result<Self, Self::Error> {
        Self::from_shared(t)
    }
}

impl TryFrom<String> for Endpoint {
    type Error = Error;

    fn try_from(t: String) -> Result<Self, Self::Error> {
        Self::from_shared(t.into_bytes())
    }
}

impl TryFrom<&'static str> for Endpoint {
    type Error = Error;

    fn try_from(t: &'static str) -> Result<Self, Self::Error> {
        Self::from_shared(t.as_bytes())
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint").finish()
    }
}

impl FromStr for Endpoint {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}
