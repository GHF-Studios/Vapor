//! Runtime-independent Vapor Content vocabulary.

use crate::VaporId;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Semantic kind of one Vapor Content artifact.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum ContentKind {
    Packagepack,
    Enginepack,
    Gamepack,
    Modpack,
    Engine,
    Game,
    EngineMod,
    GameMod,
    ExtensionMod,
    Library,
}

/// Compatibility name shared by SDK and Launcher command contracts.
pub type ContentType = ContentKind;

/// Source domain a content query should inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
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

const PACKAGEPACK_CHILDREN: &[ContentType] = &[
    ContentType::Packagepack,
    ContentType::Enginepack,
    ContentType::Gamepack,
    ContentType::Modpack,
    ContentType::Engine,
    ContentType::Game,
    ContentType::EngineMod,
    ContentType::GameMod,
    ContentType::ExtensionMod,
];
const ENGINEPACK_CHILDREN: &[ContentType] = &[
    ContentType::Engine,
    ContentType::EngineMod,
    ContentType::Enginepack,
];
const GAMEPACK_CHILDREN: &[ContentType] = &[
    ContentType::Game,
    ContentType::GameMod,
    ContentType::Gamepack,
];
const MODPACK_CHILDREN: &[ContentType] = &[
    ContentType::EngineMod,
    ContentType::GameMod,
    ContentType::ExtensionMod,
    ContentType::Modpack,
];

/// Child kinds a pack may reference before validating a complete graph.
pub fn allowed_pack_children(parent: ContentType) -> Option<&'static [ContentType]> {
    match parent {
        ContentType::Packagepack => Some(PACKAGEPACK_CHILDREN),
        ContentType::Enginepack => Some(ENGINEPACK_CHILDREN),
        ContentType::Gamepack => Some(GAMEPACK_CHILDREN),
        ContentType::Modpack => Some(MODPACK_CHILDREN),
        _ => None,
    }
}

impl ContentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Packagepack => "packagepack",
            Self::Enginepack => "enginepack",
            Self::Gamepack => "gamepack",
            Self::Modpack => "modpack",
            Self::Engine => "engine",
            Self::Game => "game",
            Self::EngineMod => "engine-mod",
            Self::GameMod => "game-mod",
            Self::ExtensionMod => "extension-mod",
            Self::Library => "library",
        }
    }
}

impl fmt::Display for ContentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod compatibility_tests {
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

/// One authored Vapor dependency declaration.
///
/// The local dependency alias is owned by the containing manifest rather than
/// by the dependency itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySpec {
    pub id: VaporId,
    pub version: VersionReq,
}
