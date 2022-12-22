use super::connection::Connection;
use super::{super::service, QuicConnector};
use crate::transport::Endpoint;

use std::{
    hash::Hash,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::Receiver;

use tokio_stream::Stream;
use tower::discover::Change;

type DiscoverResult<K, S, E> = Result<Change<K, S>, E>;

pub(crate) struct DynamicServiceStream<K: Hash + Eq + Clone> {
    changes: Receiver<Change<K, Endpoint>>,
}

impl<K: Hash + Eq + Clone> DynamicServiceStream<K> {
    pub(crate) fn new(changes: Receiver<Change<K, Endpoint>>) -> Self {
        Self { changes }
    }
}

impl<K: Hash + Eq + Clone> Stream for DynamicServiceStream<K> {
    type Item = DiscoverResult<K, Connection, crate::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let c = &mut self.changes;
        match Pin::new(&mut *c).poll_recv(cx) {
            Poll::Pending | Poll::Ready(None) => Poll::Pending,
            Poll::Ready(Some(change)) => match change {
                Change::Insert(k, endpoint) => {
                    let quic = QuicConnector::new(None);

                    #[cfg(feature = "tls")]
                    let connector = service::connector(quic, endpoint.tls.clone());

                    #[cfg(not(feature = "tls"))]
                    let connector = service::connector(quic);

                    let connection = Connection::lazy(connector, endpoint);
                    let change = Ok(Change::Insert(k, connection));
                    Poll::Ready(Some(change))
                }
                Change::Remove(k) => Poll::Ready(Some(Ok(Change::Remove(k)))),
            },
        }
    }
}

impl<K: Hash + Eq + Clone> Unpin for DynamicServiceStream<K> {}
