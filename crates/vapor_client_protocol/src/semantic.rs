use serde::{Deserialize, Serialize};
use std::{ops::Range, path::PathBuf};

/// Domain-neutral identity for an addressable object inside a workspace document.
///
/// The identity names the semantic path, while [`SemanticObjectRef::document_version`]
/// guards source ranges against edits that happened after the object was indexed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SemanticObjectId {
    pub domain: String,
    pub document: PathBuf,
    pub path: Vec<String>,
    /// Optional per-symbol instance key for same-named declarations in one scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,
}

impl SemanticObjectId {
    pub fn new(domain: impl Into<String>, document: impl Into<PathBuf>, path: Vec<String>) -> Self {
        Self {
            domain: domain.into(),
            document: document.into(),
            path,
            discriminator: None,
        }
    }

    pub fn with_discriminator(
        domain: impl Into<String>,
        document: impl Into<PathBuf>,
        path: Vec<String>,
        discriminator: impl Into<String>,
    ) -> Self {
        Self {
            domain: domain.into(),
            document: document.into(),
            path,
            discriminator: Some(discriminator.into()),
        }
    }

    pub fn qualified_name(&self) -> String {
        if self.domain == "rust" {
            return self.path.join("::");
        }
        self.path
            .iter()
            .fold(String::new(), |mut qualified, segment| {
                if segment.starts_with('[') {
                    qualified.push_str(segment);
                } else {
                    if !qualified.is_empty() {
                        qualified.push('.');
                    }
                    qualified.push_str(segment);
                }
                qualified
            })
    }
}

/// A source range in the language server's zero-based UTF-16 coordinate space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticRange {
    pub start: SemanticPosition,
    pub end: SemanticPosition,
}

/// Convert a UTF-8 byte span in a source document into the UTF-16 positions
/// used by Language Server Protocol messages.
pub fn semantic_range_from_byte_span(source: &str, span: Range<usize>) -> SemanticRange {
    fn position(source: &str, byte_offset: usize) -> SemanticPosition {
        let mut offset = byte_offset.min(source.len());
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let prefix = &source[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
        let line_start = prefix.rfind('\n').map_or(0, |newline| newline + 1);
        SemanticPosition {
            line: line.min(u32::MAX as usize) as u32,
            character: source[line_start..offset]
                .encode_utf16()
                .count()
                .min(u32::MAX as usize) as u32,
        }
    }
    SemanticRange {
        start: position(source, span.start),
        end: position(source, span.end),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticObjectRef {
    pub id: SemanticObjectId,
    pub document_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticObjectKind {
    Workspace,
    Package,
    Module,
    Struct,
    Component,
    Resource,
    Event,
    System,
    RunCondition,
    Enum,
    Variant,
    Trait,
    Function,
    Method,
    Parameter,
    Field,
    Constant,
    Static,
    TypeAlias,
    Macro,
    Attribute,
    Configuration,
    Array,
    Script,
    Other,
}

impl SemanticObjectKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Package => "Package",
            Self::Module => "Module",
            Self::Struct => "Struct",
            Self::Component => "Component",
            Self::Resource => "Resource",
            Self::Event => "Event",
            Self::System => "System",
            Self::RunCondition => "Run condition",
            Self::Enum => "Enum",
            Self::Variant => "Variant",
            Self::Trait => "Trait",
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Parameter => "Parameter",
            Self::Field => "Field",
            Self::Constant => "Constant",
            Self::Static => "Static",
            Self::TypeAlias => "Type alias",
            Self::Macro => "Macro",
            Self::Attribute => "Attribute",
            Self::Configuration => "Configuration",
            Self::Array => "Array",
            Self::Script => "Script",
            Self::Other => "Symbol",
        }
    }
}

/// A semantic node produced by a domain adapter such as Rust Analyzer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticObject {
    pub id: SemanticObjectId,
    pub kind: SemanticObjectKind,
    pub name: String,
    pub detail: Option<String>,
    pub range: SemanticRange,
    pub selection_range: SemanticRange,
    pub attributes: Vec<String>,
    pub children: Vec<SemanticObject>,
}

impl SemanticObject {
    pub fn reference(&self, document_version: u64) -> SemanticObjectRef {
        SemanticObjectRef {
            id: self.id.clone(),
            document_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_objects_round_trip_as_a_protocol_contract() {
        let object = SemanticObject {
            id: SemanticObjectId::with_discriminator(
                "rust",
                "src/components.rs",
                vec!["Ship".to_owned()],
                "line:2:component",
            ),
            kind: SemanticObjectKind::Component,
            name: "Ship".to_owned(),
            detail: Some("pub struct Ship".to_owned()),
            range: SemanticRange::default(),
            selection_range: SemanticRange::default(),
            attributes: vec!["#[derive(Component)]".to_owned()],
            children: Vec::new(),
        };

        let encoded = serde_json::to_string(&object).expect("encode semantic contract");
        let decoded: SemanticObject =
            serde_json::from_str(&encoded).expect("decode semantic contract");

        assert_eq!(decoded, object);
        assert_eq!(decoded.kind.label(), "Component");
    }
}
