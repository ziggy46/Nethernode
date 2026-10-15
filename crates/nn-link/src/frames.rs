//! The transport the link runs over: whole binary frames in each direction
//! (WebSocket binary messages, in practice).

use bytes::Bytes;
use futures_util::{Sink, Stream};

/// Binary frames in and out. One frame carries one Noise message.
pub trait FrameTransport:
    Stream<Item = std::io::Result<Bytes>> + Sink<Bytes, Error = std::io::Error> + Send + Unpin
{
}

impl<T> FrameTransport for T where
    T: Stream<Item = std::io::Result<Bytes>> + Sink<Bytes, Error = std::io::Error> + Send + Unpin
{
}

/// A boxed [`FrameTransport`].
pub type Frames = Box<dyn FrameTransport>;
