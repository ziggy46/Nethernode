//! Decoding errors.

/// Why bytes couldn't be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// More bytes are needed. Not an error for streaming readers.
    #[error("incomplete input")]
    Incomplete,
    /// A VarInt used more than 5 bytes (or a VarLong more than 10).
    #[error("VarInt too long")]
    VarIntTooLong,
    /// A length field was negative.
    #[error("negative length")]
    NegativeLength,
    /// A frame declared a length above the allowed maximum.
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    FrameTooLarge {
        /// Declared length.
        len: usize,
        /// Allowed maximum.
        max: usize,
    },
    /// A frame declared zero length (it must at least hold a packet ID).
    #[error("empty frame")]
    EmptyFrame,
    /// A string exceeded its maximum length.
    #[error("string longer than {max} characters")]
    StringTooLong {
        /// Maximum allowed, in UTF-16 code units.
        max: usize,
    },
    /// A string wasn't valid UTF-8.
    #[error("invalid UTF-8 in string")]
    InvalidUtf8,
    /// The packet ID didn't match what this state expects.
    #[error("unexpected packet id {0:#04x}")]
    UnexpectedPacketId(i32),
    /// The handshake's next state wasn't 1, 2, or 3.
    #[error("invalid next state {0}")]
    InvalidNextState(i32),
    /// A field held a value outside its valid range.
    #[error("invalid value for {0}")]
    InvalidValue(&'static str),
}

/// Result alias for decoding.
pub type Result<T, E = DecodeError> = std::result::Result<T, E>;
