//! This module provides basic networking interfaces.

mod error;
pub mod node;
mod stream_cancel;
pub mod tcp_tls;
#[cfg(test)]
pub mod test_tls_certificates;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub mod tls_test_support;

pub use error::CommsError;
pub use node::Node;
pub use tcp_tls::{TcpTlsConfig, TcpTlsConnector, TcpTlsListner};

use bytes::Bytes;
use std::net::SocketAddr;

pub type Result<T> = std::result::Result<T, CommsError>;

/// Events from peer.
#[derive(Debug)]
pub enum Event {
    NewFrame { peer: SocketAddr, frame: Bytes },
}
