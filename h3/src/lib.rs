//! HTTP/3 client and server
#![deny(missing_docs, clippy::self_named_module_files)]
#![allow(clippy::derive_partial_eq_without_eq)]

pub mod client;
pub mod error;
pub mod quic;
pub mod server;

pub use error::Error;

mod buf;
mod connection;
mod frame;
#[allow(missing_docs)]
pub mod proto;
#[allow(dead_code)]
#[allow(missing_docs)]
pub mod qpack;
#[allow(missing_docs)]
pub mod stream;

#[cfg(test)]
mod tests;

#[cfg(test)]
extern crate self as h3;