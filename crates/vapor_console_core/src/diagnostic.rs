use serde::{Deserialize, Serialize};

/// Origin of a structured diagnostic, independent of the frontend presenting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticOrigin {
    RustAnalyzer,
    Cargo,
    Rustc,
    Rustdoc,
    Vapor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// Breadth of the context a diagnostic belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticScope {
    /// Older or generic diagnostics that have no narrower semantic scope.
    #[default]
    Unspecified,
    Workspace,
    Document,
    SemanticObject,
    Operation,
}

impl DiagnosticScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Workspace => "workspace",
            Self::Document => "document",
            Self::SemanticObject => "semantic-object",
            Self::Operation => "operation",
        }
    }
}

/// Stable domain and source context shared by every diagnostic consumer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticContext {
    /// Open semantic domain name, such as `rust`, `cargo`, or `vapor-packagepack`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Breadth of the entity or operation this diagnostic describes.
    #[serde(default)]
    pub scope: DiagnosticScope,
    /// Canonical workspace root that owns the source, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Producer document revision used to reject stale source anchors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_version: Option<u64>,
    /// Domain-local qualified identity of the affected semantic object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_path: Option<String>,
    /// Stable operation identifier that produced the diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

impl DiagnosticSeverity {
    /// Map semantic severity to the palette and filtering level shared by all consoles.
    pub const fn console_level(self) -> super::ConsoleLevel {
        match self {
            Self::Error => super::ConsoleLevel::Error,
            Self::Warning => super::ConsoleLevel::Warn,
            Self::Information => super::ConsoleLevel::Info,
            Self::Hint => super::ConsoleLevel::Debug,
        }
    }
}

/// A source location supplied by a compiler or language server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticSpan {
    /// Path or URI as supplied by the producer.
    pub file: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub primary: bool,
    pub label: Option<String>,
}

/// Normalized diagnostic shared by the managed operation channel and consoles.
/// `raw_payload` preserves the producer's original record for detail views and
/// future adapters; ordinary UI can use `message` and the source anchors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredDiagnostic {
    pub origin: DiagnosticOrigin,
    pub severity: DiagnosticSeverity,
    pub code: Option<String>,
    pub message: String,
    pub rendered: Option<String>,
    pub primary: Option<DiagnosticSpan>,
    pub related: Vec<DiagnosticSpan>,
    pub raw_payload: String,
    /// Domain/scope metadata that does not depend on the frontend rendering it.
    #[serde(default)]
    pub context: DiagnosticContext,
}

#[cfg(test)]
mod tests {
    use super::DiagnosticSeverity;

    #[test]
    fn severity_maps_to_shared_console_levels() {
        assert_eq!(DiagnosticSeverity::Error.console_level(), super::super::ConsoleLevel::Error);
        assert_eq!(DiagnosticSeverity::Warning.console_level(), super::super::ConsoleLevel::Warn);
        assert_eq!(DiagnosticSeverity::Information.console_level(), super::super::ConsoleLevel::Info);
        assert_eq!(DiagnosticSeverity::Hint.console_level(), super::super::ConsoleLevel::Debug);
    }
}
