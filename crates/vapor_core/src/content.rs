//! Command-facing content source options and compatibility exports.

use clap::ValueEnum;
use std::{fmt, str::FromStr};

pub use vapor_content::{
    ContentKind, ContentType, DependencySpec, allowed_pack_children, dependency_kind_allowed,
};

/// Source domain a content query should inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ContentSource {
    Discovered,
    Local,
    Git,
    Workshop,
    All,
}

impl ContentSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Local => "local",
            Self::Git => "git",
            Self::Workshop => "workshop",
            Self::All => "all",
        }
    }
}

impl fmt::Display for ContentSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ContentSource {
    type Err = ParseContentSourceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "discovered" => Ok(Self::Discovered),
            "local" => Ok(Self::Local),
            "git" => Ok(Self::Git),
            "workshop" => Ok(Self::Workshop),
            "all" => Ok(Self::All),
            _ => Err(ParseContentSourceError(value.to_owned())),
        }
    }
}

/// Invalid content source selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseContentSourceError(String);

impl fmt::Display for ParseContentSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown content source `{}`", self.0)
    }
}

impl std::error::Error for ParseContentSourceError {}

/// A child content reference carried by authoring and launcher commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildContentRef {
    pub content_type: ContentType,
    pub content_id: String,
}

#[cfg(test)]
mod tests {
    use super::{ContentSource, ContentType, allowed_pack_children};

    #[test]
    fn shared_type_alias_preserves_pack_child_rules() {
        let children = allowed_pack_children(ContentType::Enginepack).unwrap();
        assert!(children.contains(&ContentType::Engine));
        assert!(children.contains(&ContentType::EngineMod));
        assert!(!children.contains(&ContentType::Game));
    }

    #[test]
    fn source_vocabulary_keeps_stable_cli_spellings() {
        assert_eq!(
            "workshop".parse::<ContentSource>().unwrap(),
            ContentSource::Workshop
        );
        assert_eq!(ContentSource::Discovered.to_string(), "discovered");
    }
}
