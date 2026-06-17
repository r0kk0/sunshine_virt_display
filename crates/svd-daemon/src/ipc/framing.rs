/// Newline-delimited JSON framing for the svd-daemon IPC channel.
///
/// Protocol:
///   - One JSON object per line, newline-terminated (`\n`), UTF-8.
///   - Maximum frame size: 4096 bytes total, including the trailing `\n`.
///
/// These functions are consumed by the IPC server in T3.2.
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
///
/// Consumed by the IPC server (T3.2).
#[allow(dead_code)]
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
///
/// Consumed by the IPC server (T3.2).
#[allow(dead_code)]
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

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ── 1. write_frame → read_frame round-trip (basic JSON value) ────────────

    #[test]
    fn roundtrip_basic() {
        let value = serde_json::json!({"cmd": "status"});
        let mut buf = Vec::new();
        write_frame(&mut buf, &value).expect("write_frame failed");

        // Verify the wire format ends with '\n'.
        assert_eq!(buf.last(), Some(&b'\n'));

        let mut cursor = Cursor::new(buf);
        let frame = read_frame(&mut cursor).expect("read_frame failed");

        let parsed: serde_json::Value =
            serde_json::from_str(&frame).expect("frame not valid JSON");
        assert_eq!(parsed, value);
    }

    // ── 2. Read multiple frames from the same reader in sequence ─────────────

    #[test]
    fn multiple_frames_sequential() {
        let v1 = serde_json::json!({"cmd": "connect"});
        let v2 = serde_json::json!({"cmd": "disconnect"});
        let v3 = serde_json::json!({"cmd": "status"});

        let mut buf = Vec::new();
        write_frame(&mut buf, &v1).unwrap();
        write_frame(&mut buf, &v2).unwrap();
        write_frame(&mut buf, &v3).unwrap();

        let mut cursor = Cursor::new(buf);
        let f1 = read_frame(&mut cursor).unwrap();
        let f2 = read_frame(&mut cursor).unwrap();
        let f3 = read_frame(&mut cursor).unwrap();

        assert_eq!(serde_json::from_str::<serde_json::Value>(&f1).unwrap(), v1);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&f2).unwrap(), v2);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&f3).unwrap(), v3);
    }

    // ── 3. Frame exactly MAX_FRAME_SIZE-1 content bytes → Ok ─────────────────
    //
    // Wire size: MAX_FRAME_SIZE-1 content bytes + '\n' = MAX_FRAME_SIZE bytes.
    // write_frame guard: serialized.len() + 1 = MAX_FRAME_SIZE  → allowed
    //   (condition is: > MAX_FRAME_SIZE, so == is Ok).
    // read_frame guard: buf grows to MAX_FRAME_SIZE-1 before '\n' hits,
    //   so buf.len() >= MAX_FRAME_SIZE is never true before the newline → Ok.

    #[test]
    fn frame_exactly_max_minus_one_content_bytes_ok() {
        // Build a raw frame of MAX_FRAME_SIZE-1 'a' bytes followed by '\n'.
        let content_len = MAX_FRAME_SIZE - 1; // 4095
        let mut wire: Vec<u8> = vec![b'a'; content_len];
        wire.push(b'\n');

        let mut cursor = Cursor::new(wire);
        let result = read_frame(&mut cursor);
        assert!(result.is_ok(), "expected Ok for {content_len}-byte content frame");
        assert_eq!(result.unwrap().len(), content_len);
    }

    // ── 4. write_frame with value too large → Err(TooLarge) ──────────────────

    #[test]
    fn write_frame_too_large_rejected() {
        // A string value whose JSON serialization pushes total wire size > MAX_FRAME_SIZE.
        // serde_json serializes a String with surrounding quotes, so we need
        // len(json) + 1 > 4096, i.e. len(json) >= 4096.
        // json = '"' + content + '"', so content must be ≥ 4094 bytes.
        let big_string = "x".repeat(MAX_FRAME_SIZE); // definitely > 4096 with quotes + newline
        let value = serde_json::json!(big_string);
        let mut buf = Vec::new();
        let result = write_frame(&mut buf, &value);
        assert!(
            matches!(result, Err(FrameError::TooLarge)),
            "expected TooLarge, got {result:?}"
        );
    }

    // ── 5. EOF before newline → Err(ConnectionClosed) ────────────────────────

    #[test]
    fn eof_before_newline_is_connection_closed() {
        // Stream has data but no terminating '\n'.
        let data = b"incomplete_frame_no_newline";
        let mut cursor = Cursor::new(data.as_ref());
        let result = read_frame(&mut cursor);
        assert!(
            matches!(result, Err(FrameError::ConnectionClosed)),
            "expected ConnectionClosed, got {result:?}"
        );
    }

    // ── 5b. Empty EOF → Err(ConnectionClosed) ────────────────────────────────

    #[test]
    fn empty_eof_is_connection_closed() {
        let mut cursor = Cursor::new(&[][..]);
        let result = read_frame(&mut cursor);
        assert!(
            matches!(result, Err(FrameError::ConnectionClosed)),
            "expected ConnectionClosed on empty EOF, got {result:?}"
        );
    }

    // ── 6. Over-size frame before newline → Err(TooLarge) ────────────────────
    //
    // Feed MAX_FRAME_SIZE bytes of content without a newline — the buffer
    // hits `>= MAX_FRAME_SIZE` and returns TooLarge before EOF.

    #[test]
    fn oversize_read_frame_rejected() {
        // MAX_FRAME_SIZE content bytes + '\n' at the end (so it's not just EOF).
        let mut wire: Vec<u8> = vec![b'b'; MAX_FRAME_SIZE];
        wire.push(b'\n');

        let mut cursor = Cursor::new(wire);
        let result = read_frame(&mut cursor);
        assert!(
            matches!(result, Err(FrameError::TooLarge)),
            "expected TooLarge for {MAX_FRAME_SIZE}-content-byte frame, got {result:?}"
        );
    }

    // ── 7. Round-trip with svd_proto::Request ─────────────────────────────────

    #[test]
    fn roundtrip_proto_request_status() {
        let req = svd_proto::Request::Status {};
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).expect("write_frame for Request::Status failed");

        let mut cursor = Cursor::new(buf);
        let frame = read_frame(&mut cursor).expect("read_frame failed");

        let parsed: svd_proto::Request =
            serde_json::from_str(&frame).expect("frame not valid Request JSON");
        assert!(
            matches!(parsed, svd_proto::Request::Status { .. }),
            "unexpected variant after round-trip: {parsed:?}"
        );
    }
}
