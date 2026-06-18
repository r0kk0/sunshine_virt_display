//! Newline-delimited JSON framing for the svd IPC channel.
//!
//! Protocol:
//!   - One JSON object per line, newline-terminated (`\n`), UTF-8.
//!   - Maximum frame size: 4096 bytes total, including the trailing `\n`.
//!
//! Used by both the daemon (server side) and the CLI (client side).

use std::io::{Read, Write};

/// Maximum IPC frame size in bytes, including the trailing newline.
pub const MAX_FRAME_SIZE: usize = 4096;

/// Errors that can occur during frame read/write.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame exceeds maximum size of 4096 bytes")]
    TooLarge,
    #[error("connection closed before frame complete")]
    ConnectionClosed,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("serialization error: {0}")]
    Serialize(String),
}

/// Read one newline-delimited JSON frame from `reader`.
///
/// Returns `Ok(String)` with the frame content (without the trailing `\n`).
/// Returns `Err` if the frame exceeds `MAX_FRAME_SIZE`, contains invalid
/// UTF-8, or the connection is closed before a complete frame is received.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<String, FrameError> {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];

    loop {
        let n = reader.read(&mut byte)?;
        if n == 0 {
            // EOF: whether or not we have partial data, the frame is incomplete.
            return Err(FrameError::ConnectionClosed);
        }

        if byte[0] == b'\n' {
            // Frame complete — convert to String (without the newline).
            return Ok(String::from_utf8(buf)?);
        }

        buf.push(byte[0]);

        // After pushing, buf.len() is the count of content bytes (no newline
        // yet).  A valid full frame is at most MAX_FRAME_SIZE-1 content bytes
        // plus one '\n'.  If we already have MAX_FRAME_SIZE bytes of content
        // (and still no newline), the frame is over-size.
        if buf.len() >= MAX_FRAME_SIZE {
            return Err(FrameError::TooLarge);
        }
    }
}

/// Write a JSON value as a newline-terminated frame to `writer`.
///
/// Returns `Err(TooLarge)` if the serialized form (plus 1 byte for `\n`)
/// would exceed `MAX_FRAME_SIZE`.
pub fn write_frame<W: Write, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), FrameError> {
    let serialized =
        serde_json::to_string(value).map_err(|e| FrameError::Serialize(e.to_string()))?;

    // serialized.len() is content bytes; +1 for the trailing '\n'.
    if serialized.len() + 1 > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge);
    }

    writer.write_all(serialized.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(())
}
