//! Parsing of machine-readable Cargo compiler output.

use vapor_core::{
    DiagnosticContext, DiagnosticOrigin, DiagnosticScope, DiagnosticSeverity, DiagnosticSpan,
    StructuredDiagnostic,
};

const MAX_RETAINED_CARGO_PAYLOAD_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoOutputLine {
    pub display_text: String,
    pub diagnostic: Option<StructuredDiagnostic>,
    /// Original Cargo record, retained when display text is summarized.
    pub raw_payload: Option<String>,
}

/// Normalize one Cargo JSON output line while retaining the original record
/// inside any structured compiler diagnostic. Non-diagnostic lines stay raw.
pub fn parse_cargo_output_line(line: &str) -> CargoOutputLine {
    let Some(value) = serde_json::from_str::<serde_json::Value>(line).ok() else {
        return raw_line(line);
    };
    let reason = value.get("reason").and_then(serde_json::Value::as_str);
    if reason != Some("compiler-message") {
        return match reason {
            Some("compiler-artifact") => {
                let target = value
                    .get("target")
                    .and_then(|target| target.get("name"))
                    .and_then(serde_json::Value::as_str);
                let kind = value
                    .get("target")
                    .and_then(|target| target.get("kind"))
                    .and_then(serde_json::Value::as_array)
                    .and_then(|kinds| kinds.first())
                    .and_then(serde_json::Value::as_str);
                match (target, kind) {
                    (Some(target), Some(kind)) => {
                        summarized_line(&format!("Built {kind} {target}"), line)
                    }
                    (Some(target), None) => summarized_line(&format!("Built {target}"), line),
                    _ => summarized_line("Cargo built a target", line),
                }
            }
            Some("build-script-executed") => {
                let package = value
                    .get("package_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("package");
                summarized_line(&format!("Ran build script for {package}"), line)
            }
            Some("build-finished") => {
                if value.get("success").and_then(serde_json::Value::as_bool) == Some(true) {
                    summarized_line("Cargo finished successfully", line)
                } else {
                    summarized_line("Cargo reported a failed build", line)
                }
            }
            Some(_) => raw_line(line),
            None => raw_line(line),
        };
    }
    let Some(message) = value.get("message") else {
        return raw_line(line);
    };
    let Some(text) = message.get("message").and_then(serde_json::Value::as_str) else {
        return raw_line(line);
    };
    let severity = match message.get("level").and_then(serde_json::Value::as_str) {
        Some("error") => DiagnosticSeverity::Error,
        Some("warning") => DiagnosticSeverity::Warning,
        Some("help") => DiagnosticSeverity::Hint,
        Some("note" | "failure-note") => DiagnosticSeverity::Information,
        _ => return raw_line(line),
    };
    let mut spans = message
        .get("spans")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_span)
        .collect::<Vec<_>>();
    let primary_index = spans.iter().position(|span| span.primary);
    let primary = primary_index.map(|index| spans.remove(index));
    let related = spans;
    let rendered = message
        .get("rendered")
        .and_then(serde_json::Value::as_str)
        .filter(|rendered| !rendered.trim().is_empty())
        .map(str::to_owned);
    let display_text = rendered
        .as_ref()
        .map(|rendered| rendered.trim_end().to_owned())
        .unwrap_or_else(|| text.to_owned());
    let code = message
        .get("code")
        .and_then(|code| code.get("code"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let scope = if primary.is_some() {
        DiagnosticScope::Document
    } else {
        DiagnosticScope::Operation
    };
    CargoOutputLine {
        display_text,
        diagnostic: Some(StructuredDiagnostic {
            origin: DiagnosticOrigin::Cargo,
            severity,
            code,
            message: text.to_owned(),
            rendered,
            primary,
            related,

            raw_payload: truncate_utf8(line, MAX_RETAINED_CARGO_PAYLOAD_BYTES),
            context: DiagnosticContext {
                domain: Some("rust".to_owned()),
                scope,
                ..Default::default()
            },
        }),
        raw_payload: None,
    }
}

fn parse_span(span: &serde_json::Value) -> Option<DiagnosticSpan> {
    Some(DiagnosticSpan {
        file: span.get("file_name")?.as_str()?.to_owned(),
        start_line: bounded_u32(span.get("line_start")?.as_u64()?),
        start_column: bounded_u32(span.get("column_start")?.as_u64()?),
        end_line: bounded_u32(span.get("line_end")?.as_u64()?),
        end_column: bounded_u32(span.get("column_end")?.as_u64()?),
        primary: span
            .get("is_primary")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        label: span
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

fn bounded_u32(value: u64) -> u32 {
    value.min(u32::MAX as u64) as u32
}

fn raw_line(line: &str) -> CargoOutputLine {
    CargoOutputLine {
        display_text: line.to_owned(),
        diagnostic: None,
        raw_payload: None,
    }
}

fn summarized_line(display_text: &str, raw: &str) -> CargoOutputLine {
    CargoOutputLine {
        display_text: display_text.to_owned(),
        diagnostic: None,
        raw_payload: Some(truncate_utf8(raw, MAX_RETAINED_CARGO_PAYLOAD_BYTES)),
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut retained = value[..end].to_owned();
    retained.push('…');
    retained
}

#[cfg(test)]
mod tests {
    use super::{MAX_RETAINED_CARGO_PAYLOAD_BYTES, parse_cargo_output_line};
    use vapor_core::{DiagnosticOrigin, DiagnosticSeverity};

    #[test]
    fn compiler_message_has_readable_text_provenance_and_source_spans() {
        let line = r#"{"reason":"compiler-message","message":{"message":"cannot find value `ship`","level":"error","code":{"code":"E0425"},"rendered":"error[E0425]: cannot find value `ship`\n --> src/main.rs:4:5\n  |\n4 |     ship.velocity();\n  |     ^^^^ not found\n","spans":[{"file_name":"src/main.rs","line_start":4,"line_end":4,"column_start":5,"column_end":9,"is_primary":true,"label":"not found"},{"file_name":"src/lib.rs","line_start":2,"line_end":2,"column_start":1,"column_end":4,"is_primary":false,"label":"declared here"}]}}"#;
        let output = parse_cargo_output_line(line);
        let diagnostic = output.diagnostic.expect("compiler diagnostic");

        assert_eq!(diagnostic.origin, DiagnosticOrigin::Cargo);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostic.code.as_deref(), Some("E0425"));
        assert!(output.display_text.starts_with("error[E0425]"));
        assert_eq!(diagnostic.primary.as_ref().unwrap().file, "src/main.rs");
        assert_eq!(diagnostic.related[0].file, "src/lib.rs");
        assert_eq!(diagnostic.raw_payload, line);
    }

    #[test]
    fn cargo_machine_events_are_summarized_instead_of_leaking_json() {
        let artifact =
            r#"{"reason":"compiler-artifact","target":{"name":"vapor_core","kind":["lib"]}}"#;
        let output = parse_cargo_output_line(artifact);
        assert_eq!(output.display_text, "Built lib vapor_core");
        assert!(output.diagnostic.is_none());
        assert!(!output.display_text.starts_with('{'));
        assert_eq!(output.raw_payload.as_deref(), Some(artifact));

        let finished = parse_cargo_output_line(r#"{"reason":"build-finished","success":true}"#);
        assert_eq!(finished.display_text, "Cargo finished successfully");
        assert_eq!(
            finished.raw_payload.as_deref(),
            Some(r#"{"reason":"build-finished","success":true}"#)
        );
    }

    #[test]
    fn cargo_non_diagnostic_and_unstructured_output_remains_verbatim() {
        for line in [
            "ordinary build-script output",
            r#"{"reason":"custom-event","value":1}"#,
        ] {
            let output = parse_cargo_output_line(line);
            assert_eq!(output.display_text, line);
            assert!(output.diagnostic.is_none());
        }
    }

    #[test]
    fn retained_cargo_payload_is_bounded_at_utf8_boundaries() {
        let message = "x".repeat(MAX_RETAINED_CARGO_PAYLOAD_BYTES + 100);
        let line = format!(
            "{{\"reason\":\"compiler-message\",\"message\":{{\"message\":\"{message}\",\"level\":\"warning\"}}}}"
        );
        let output = parse_cargo_output_line(&line);
        let raw = &output.diagnostic.unwrap().raw_payload;
        assert!(raw.len() <= MAX_RETAINED_CARGO_PAYLOAD_BYTES + '…'.len_utf8());
        assert!(raw.is_char_boundary(raw.len()));
        assert!(raw.ends_with('…'));
    }
}
