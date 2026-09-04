//! Runtime-independent Vapor Content vocabulary.

use crate::VaporId;
use semver::VersionReq;
use serde::{Deserialize, Serialize};

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
