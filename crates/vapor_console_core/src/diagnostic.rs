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
