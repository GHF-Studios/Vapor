//! Runtime-independent Vapor Content vocabulary.

use crate::VaporId;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Semantic kind of one Vapor Content artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
