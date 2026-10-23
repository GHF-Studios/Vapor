use std::io::{BufRead, Read};
use vapor_client_protocol::ProtocolFrame;

pub const MAX_FRAME_BYTES: usize = vapor_client_protocol::MAX_PROTOCOL_FRAME_BYTES;

pub fn read_frame(reader: &mut impl BufRead) -> Result<Option<ProtocolFrame>, String> {
    let mut bytes = Vec::new();
    let mut limit = (&mut *reader).take((MAX_FRAME_BYTES + 1) as u64);
    let read = limit
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("failed reading service output: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("service protocol frame exceeds the 32 MiB limit".to_owned());
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| {
            let first = bytes.iter().copied().find(|byte| !byte.is_ascii_whitespace());
            let kind = match first {
                Some(b'<') => "markup text",
                Some(0x1b) => "terminal control output",
                Some(b'{') | Some(b'[') => "malformed JSON",
                Some(_) => "non-JSON text",
                None => "empty data",
            };
            let first_byte = first.map_or_else(|| "none".to_owned(), |byte| format!("0x{byte:02x}"));
            format!(
                "service emitted malformed protocol data: {error} ({} bytes; {kind}; first non-whitespace byte {first_byte})",
                bytes.len(),
            )
        })
}

pub fn encode_frame(frame: &ProtocolFrame) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(frame).map_err(|error| error.to_string())?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(format!(
            "client protocol frame exceeds the {} byte limit",
            MAX_FRAME_BYTES
        ));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn clean_eof_has_no_frame() {
        assert!(
            read_frame(&mut Cursor::new(Vec::<u8>::new()))
                .expect("read clean EOF")
                .is_none()
        );
    }

    #[test]
    fn malformed_frames_fail_with_protocol_context() {
        let error =
            read_frame(&mut Cursor::new(b"{broken json}\n")).expect_err("invalid JSON must fail");
        assert!(error.contains("malformed protocol data"));
        assert!(error.contains("malformed JSON"));
        assert!(error.contains("first non-whitespace byte 0x7b"));
    }

    #[test]
    fn malformed_plain_text_is_classified_without_echoing_payload_contents() {
        let error = read_frame(&mut Cursor::new(b"password=secret\n"))
            .expect_err("plain text is not a protocol frame");
        assert!(error.contains("non-JSON text"));
        assert!(error.contains("0x70"));
        assert!(!error.contains("secret"));
    }

    #[test]
    fn oversized_frames_are_rejected_before_json_decoding() {
        let bytes = vec![b'x'; MAX_FRAME_BYTES + 1];
        let error = read_frame(&mut Cursor::new(bytes)).expect_err("oversized frame must fail");
        assert!(error.contains("32 MiB limit"));
    }

    #[test]
    fn oversized_outbound_frames_are_rejected_before_writing() {
        let frame = ProtocolFrame::Event(vapor_client_protocol::ClientEvent::ProtocolError(
            vapor_client_protocol::ServiceError {
                code: vapor_client_protocol::ServiceErrorCode::InvalidRequest,
                message: "x".repeat(MAX_FRAME_BYTES),
                retryable: false,
            },
        ));
        let error = encode_frame(&frame).expect_err("oversized frame must be rejected");
        assert!(error.contains("client protocol frame exceeds"));
    }
}
