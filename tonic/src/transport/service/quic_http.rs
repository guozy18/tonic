use bytes::{BufMut, Bytes, BytesMut};
use futures::future::poll_fn;
use futures::ready;
use futures_core::Future;
use h3::proto::{frame::Frame, headers::Header};
use h3::qpack;
use http::{request, Request, Response};
use http_body::Body as HttpBody;
use hyper::{rt::Executor, Body};
use std::convert::TryFrom;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tower::make::MakeConnection;
use tower::Service;

use super::executor;

pub(crate) struct Http3Connector<M, B> {
    inner: M,
    _marker: PhantomData<fn(B)>,
}

impl<M, B> Http3Connector<M, B> {
    pub(crate) fn new(inner: M) -> Self {
        Self {
            inner,
            _marker: PhantomData,
        }
    }
}

pub(crate) struct SendRequest<Req, Resp> {
    sender: mpsc::Sender<Req>,
    recver: Option<mpsc::Receiver<Resp>>,
}

impl<M, T, B> Service<T> for Http3Connector<M, B>
where
    M: MakeConnection<T> + Send,
    M::Connection: Unpin + Send + 'static,
    M::Future: Send + 'static,
    M::Error: Into<crate::Error> + Send,
    T: Send,
    B: HttpBody + 'static + Send + Unpin,
{
    type Response = SendRequest<Request<B>, Response<Body>>;
    type Error = crate::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: T) -> Self::Future {
        let make_conn = self.inner.make_connection(req);
        Box::pin(async move {
            // h3_quinn::Connection
            let mut conn = make_conn.await.map_err(Into::into)?;
            let (sender, mut rx) = mpsc::channel::<Request<B>>(8);
            let (tx, recver) = mpsc::channel(8);

            executor::SharedExec::tokio().execute(Box::pin(async move {
                while let Some(request) = rx.recv().await {
                    let mut send_block = BytesMut::new();
                    let mut recv_buf = vec![MaybeUninit::<u8>::uninit(); 0x4000];

                    // Receive request header
                    let (parts, mut body) = request.into_parts();
                    let request::Parts {
                        method,
                        uri,
                        headers,
                        ..
                    } = parts;
                    let headers = Header::request(method, uri, headers).unwrap();
                    let _size = qpack::encode_stateless(&mut send_block, headers).unwrap();

                    // Receive request body
                    while let Some(Ok(buf)) = poll_fn(|cx| Pin::new(&mut body).poll_data(cx)).await
                    {
                        send_block.put(buf);
                    }

                    let send_block = send_block.freeze();
                    // Send request
                    poll_fn(|cx| Pin::new(&mut conn).poll_write(cx, &send_block[..]))
                        .await
                        .unwrap();
                    poll_fn(|cx| Pin::new(&mut conn).poll_flush(cx))
                        .await
                        .unwrap();

                    let mut tx_holder = None;

                    while let Ok(mut b) =
                        poll_fn(|cx| poll_frame(cx, &mut conn, &mut recv_buf)).await
                    {
                        if let Ok(frame) = Frame::decode(&mut b) {
                            match frame {
                                Frame::Headers(mut b) => {
                                    let (body_tx, body) = Body::channel();
                                    let mut resp = Response::new(body);
                                    tx_holder = Some(body_tx);

                                    let qpack::Decoded { fields, .. } =
                                        qpack::decode_stateless(&mut b, 0x4000).unwrap();
                                    let (status, headers) = Header::try_from(fields)
                                        .unwrap()
                                        .into_response_parts()
                                        .unwrap();
                                    *resp.status_mut() = status;
                                    *resp.headers_mut() = headers;
                                    *resp.version_mut() = http::Version::HTTP_3;
                                    tx.send(resp).await.unwrap();
                                }
                                Frame::Data(_size) => {
                                    if let Some(ref mut body_tx) = tx_holder {
                                        body_tx.send_data(b).await.unwrap();
                                    }
                                }
                                Frame::CancelPush(_id) => {}
                                _ => {}
                            }
                        }
                    } // while poll_read
                } // while recv from rx
            }));
            Ok(SendRequest {
                sender,
                recver: Some(recver),
            })
        })
    }
}

impl<Req, Resp> Clone for SendRequest<Req, Resp> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            recver: None,
        }
    }
}

impl<HB> Service<Request<HB>> for SendRequest<Request<HB>, Response<Body>>
where
    HB: HttpBody + 'static + Send + Debug,
{
    type Response = Response<Body>;
    type Error = crate::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<HB>) -> Self::Future {
        let sender = self.sender.clone();
        let mut recver = self.recver.take().unwrap();
        Box::pin(async move {
            sender.send(req).await.unwrap();
            let resp = recver.recv().await.unwrap();
            Ok(resp)
        })
    }
}

fn poll_frame<T>(
    cx: &mut Context<'_>,
    mut conn: &mut T,
    buf: &mut Vec<MaybeUninit<u8>>,
) -> Poll<Result<Bytes, crate::Error>>
where
    T: AsyncRead + Unpin,
{
    let mut read_buf = ReadBuf::uninit(&mut buf[..]);
    ready!(Pin::new(&mut conn).poll_read(cx, &mut read_buf))?;
    Poll::Ready(Ok(Bytes::copy_from_slice(read_buf.filled())))
}
