//! IPC framing — re-exported from svd-proto so that both the daemon and the
//! CLI share the same implementation without duplicating code.
//!
//! All public symbols (`MAX_FRAME_SIZE`, `FrameError`, `read_frame`,
//! `write_frame`) are available directly from this module via the glob
//! re-export below.

pub use svd_proto::framing::*;

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

        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("frame not valid JSON");
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
        assert!(
            result.is_ok(),
            "expected Ok for {content_len}-byte content frame"
        );
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
