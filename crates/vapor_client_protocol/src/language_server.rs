use std::{convert::TryFrom, io::{self, Write}};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vapor_console_core::{
    DiagnosticOrigin, DiagnosticSeverity, DiagnosticSpan, StructuredDiagnostic,
};

const MAX_DIAGNOSTICS_PER_UPDATE: usize = 256;
const MAX_DIAGNOSTIC_TEXT_BYTES: usize = 16 * 1024;
const MAX_DIAGNOSTIC_RAW_BYTES: usize = 16 * 1024;
const MAX_RELATED_LOCATIONS: usize = 8;
const MAX_LOCATION_URI_BYTES: usize = 4 * 1024;
const MAX_LOCATION_LABEL_BYTES: usize = 2 * 1024;

/// Structured Rust Analyzer diagnostics emitted for one document revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageServerDiagnostics {
    pub uri: String,
    pub version: Option<i32>,
    pub diagnostics: Vec<StructuredDiagnostic>,
    /// Diagnostics omitted because they were malformed or exceeded the per-update bound.
    #[serde(default)]
    pub omitted_diagnostics: usize,
}

/// Normalize an LSP `textDocument/publishDiagnostics` notification at the
/// service boundary so every frontend consumes the same diagnostic contract.
pub fn parse_publish_diagnostics(message: &Value) -> Option<LanguageServerDiagnostics> {
    if message.get("method").and_then(Value::as_str) != Some("textDocument/publishDiagnostics") {
        return None;
    }
    let params = message.get("params")?;
    let uri = params.get("uri")?.as_str()?;
    if uri.len() > MAX_LOCATION_URI_BYTES {
        return None;
    }
    let raw_diagnostics = params.get("diagnostics")?.as_array()?;
    let version = params
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|version| i32::try_from(version).ok());
    let mut diagnostics = Vec::with_capacity(raw_diagnostics.len().min(MAX_DIAGNOSTICS_PER_UPDATE));
    for raw in raw_diagnostics.iter().take(MAX_DIAGNOSTICS_PER_UPDATE) {
        if let Some(diagnostic) = parse_diagnostic(
            raw,
            uri,
            version.and_then(|version| u64::try_from(version).ok()),
        ) {
            diagnostics.push(diagnostic);
        }
    }
    let omitted_diagnostics = raw_diagnostics.len().saturating_sub(diagnostics.len());
    Some(LanguageServerDiagnostics {
        uri: uri.to_owned(),
        version,
        diagnostics,
        omitted_diagnostics,
    })
}

fn parse_diagnostic(
    diagnostic: &Value,
    uri: &str,
    document_version: Option<u64>,
) -> Option<StructuredDiagnostic> {
    let message = diagnostic.get("message")?.as_str()?;
    let primary = parse_span(uri, diagnostic.get("range")?, None, true);
    let primary = primary?;
    let related_information = diagnostic
        .get("relatedInformation")
        .and_then(Value::as_array);
    let mut related = Vec::new();
    if let Some(items) = related_information {
        for information in items.iter().take(MAX_RELATED_LOCATIONS) {
            let Some(location) = information.get("location") else { continue };
            let Some(related_uri) = location.get("uri").and_then(Value::as_str) else { continue };
            if related_uri.len() > MAX_LOCATION_URI_BYTES {
                continue;
            }
            let label = information
                .get("message")
                .and_then(Value::as_str)
                .map(|label| truncate(label, MAX_LOCATION_LABEL_BYTES));
            let Some(range) = location.get("range") else { continue };
            if let Some(span) = parse_span(related_uri, range, label, false) {
                related.push(span);
            }
        }
    }
    let severity = match diagnostic.get("severity").and_then(Value::as_u64) {
        Some(1) => DiagnosticSeverity::Error,
        Some(2) => DiagnosticSeverity::Warning,
        Some(3) => DiagnosticSeverity::Information,
        Some(4) => DiagnosticSeverity::Hint,
        _ => DiagnosticSeverity::Information,
    };
    let code = diagnostic.get("code").and_then(|code| {
        if code.is_null() {
            None
        } else {
            Some(code.as_str().map(str::to_owned).unwrap_or_else(|| code.to_string()))
        }
    });
    let raw_payload = serialize_bounded_json(diagnostic, MAX_DIAGNOSTIC_RAW_BYTES);
    Some(StructuredDiagnostic {
        origin: DiagnosticOrigin::RustAnalyzer,
        severity,
        code,
        message: truncate(message, MAX_DIAGNOSTIC_TEXT_BYTES),
        rendered: diagnostic
            .get("rendered")
            .and_then(Value::as_str)
            .map(|rendered| truncate(rendered, MAX_DIAGNOSTIC_TEXT_BYTES)),
        primary: Some(primary),
        related,

        raw_payload: truncate(&raw_payload, MAX_DIAGNOSTIC_RAW_BYTES),
        context: vapor_console_core::DiagnosticContext {
            domain: Some("rust".to_owned()),
            scope: vapor_console_core::DiagnosticScope::Document,
            document_version,
            ..Default::default()
        },
    })
}

/// Serialize untrusted tool data without first allocating a second copy of an
/// arbitrarily large diagnostic. A truncated payload is intentionally a
/// display-only prefix, marked so consumers do not treat it as complete JSON.
fn serialize_bounded_json(value: &Value, max_bytes: usize) -> String {
    const SUFFIX: &str = "… [truncated]";
    if max_bytes == 0 {
        return String::new();
    }
    let mut output = BoundedJsonBuffer {
        bytes: Vec::with_capacity(max_bytes.min(1024)),
        max_bytes,
    };
    if serde_json::to_writer(&mut output, value).is_ok() {
        return String::from_utf8(output.bytes).unwrap_or_default();
    }
    let prefix = String::from_utf8_lossy(&output.bytes);
    let prefix_limit = max_bytes.saturating_sub(SUFFIX.len());
    let mut end = prefix.len().min(prefix_limit);
    while !prefix.is_char_boundary(end) {
        end -= 1;
    }
    if max_bytes <= SUFFIX.len() {
        let mut suffix_end = max_bytes;
        while !SUFFIX.is_char_boundary(suffix_end) {
            suffix_end -= 1;
        }
        SUFFIX[..suffix_end].to_owned()
    } else {
        format!("{}{SUFFIX}", &prefix[..end])
    }
}

struct BoundedJsonBuffer {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl Write for BoundedJsonBuffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.max_bytes.saturating_sub(self.bytes.len());
        if remaining == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "diagnostic payload limit reached"));
        }
        let length = buffer.len().min(remaining);
        self.bytes.extend_from_slice(&buffer[..length]);
        if length < buffer.len() {
            Err(io::Error::new(io::ErrorKind::WriteZero, "diagnostic payload limit reached"))
        } else {
            Ok(length)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn parse_span(uri: &str, range: &Value, label: Option<String>, primary: bool) -> Option<DiagnosticSpan> {
    let start = range.get("start")?;
    let end = range.get("end")?;
    let coordinate = |position: &Value, field: &str| {
        position
            .get(field)?
            .as_u64()
            .map(|value| value.min(u32::MAX as u64) as u32)
            .map(|value| value.saturating_add(1))
    };
    Some(DiagnosticSpan {
        file: uri.to_owned(),
        start_line: coordinate(start, "line")?,
        start_column: coordinate(start, "character")?,
        end_line: coordinate(end, "line")?,
        end_column: coordinate(end, "character")?,
        primary,
        label,
    })
}

fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    const SUFFIX: &str = "… [truncated]";
    let mut end = max_bytes.saturating_sub(SUFFIX.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{SUFFIX}", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_diagnostics_normalizes_severity_ranges_codes_and_related_locations() {
        let message = serde_json::json!({
            "jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params": {
                "uri":"file:///work/src/lib.rs",
                "version":7,
                "diagnostics":[{
                    "range":{"start":{"line":2,"character":4},"end":{"line":2,"character":9}},
                    "severity":2,
                    "code":123,
                    "message":"misplaced type",
                    "relatedInformation":[{
                        "location":{"uri":"file:///work/src/types.rs","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":5}}},
                        "message":"canonical module"
                    }]
                }]
            }
        });

        let batch = parse_publish_diagnostics(&message).unwrap();
        assert_eq!(batch.version, Some(7));
        assert_eq!(batch.diagnostics.len(), 1);
        let diagnostic = &batch.diagnostics[0];
        assert_eq!(diagnostic.origin, DiagnosticOrigin::RustAnalyzer);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning);
        assert_eq!(diagnostic.code.as_deref(), Some("123"));
        let primary = diagnostic.primary.as_ref().unwrap();
        assert_eq!((primary.start_line, primary.start_column), (3, 5));
        assert_eq!(diagnostic.related[0].file, "file:///work/src/types.rs");
        assert_eq!(diagnostic.related[0].label.as_deref(), Some("canonical module"));
    }

    #[test]
    fn publish_diagnostics_limits_rows_and_bounds_untrusted_text() {
        let message = serde_json::json!({
            "method":"textDocument/publishDiagnostics",
            "params":{
                "uri":"file:///work/src/lib.rs",
                "diagnostics": (0..300).map(|_| serde_json::json!({
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                    "message":format!("{}🌌", "x".repeat(MAX_DIAGNOSTIC_TEXT_BYTES))
                })).collect::<Vec<_>>()
            }
        });

        let batch = parse_publish_diagnostics(&message).unwrap();
        assert_eq!(batch.diagnostics.len(), MAX_DIAGNOSTICS_PER_UPDATE);
        assert_eq!(batch.omitted_diagnostics, 300 - MAX_DIAGNOSTICS_PER_UPDATE);
        let diagnostic = &batch.diagnostics[0];
        assert!(diagnostic.message.len() <= MAX_DIAGNOSTIC_TEXT_BYTES);
        assert!(diagnostic.message.is_char_boundary(diagnostic.message.len()));
        assert!(diagnostic.message.ends_with("[truncated]"));
        assert!(diagnostic.raw_payload.len() <= MAX_DIAGNOSTIC_RAW_BYTES);
    }

    #[test]
    fn only_publish_diagnostics_notifications_are_normalized() {
        assert!(parse_publish_diagnostics(&serde_json::json!({"method":"textDocument/didOpen"})).is_none());
    }

    #[test]
    fn bounded_json_serialization_never_retains_the_full_untrusted_payload() {
        let value = serde_json::json!({"message": "x".repeat(MAX_DIAGNOSTIC_RAW_BYTES * 8)});
        let raw = serialize_bounded_json(&value, 256);

        assert!(raw.len() <= 256);
        assert!(raw.ends_with("[truncated]"));
        assert!(raw.starts_with('{'));
    }
}
