//! Runtime-independent Vapor content vocabulary.

use crate::VaporId;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Semantic kind of one Vapor Content artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContentKind {
    /// Complete runnable composition.
    Packagepack,
    /// Engine composition.
    Enginepack,
    /// Game composition.
    Gamepack,
    /// Mod collection.
    Modpack,
    /// Engine implementation.
    Engine,
    /// Game implementation.
    Game,
    /// Engine extension.
    EngineMod,
    /// Game extension.
    GameMod,
    /// General extension.
    ExtensionMod,
    /// Rust-only statically linked code.
    Crate,
}

impl ContentKind {
    /// Every content kind in canonical manifest-discovery order.
    pub const ALL: [Self; 10] = [
        Self::Packagepack,
        Self::Enginepack,
        Self::Gamepack,
        Self::Modpack,
        Self::Engine,
        Self::Game,
        Self::EngineMod,
        Self::GameMod,
        Self::ExtensionMod,
        Self::Crate,
    ];

    /// Canonical role-specific source manifest filename.
    pub const fn manifest_file_name(self) -> &'static str {
        match self {
            Self::Packagepack => "Packagepack.vapor.toml",
            Self::Enginepack => "Enginepack.vapor.toml",
            Self::Gamepack => "Gamepack.vapor.toml",
            Self::Modpack => "Modpack.vapor.toml",
            Self::Engine => "Engine.vapor.toml",
            Self::Game => "Game.vapor.toml",
            Self::EngineMod => "Engine-Mod.vapor.toml",
            Self::GameMod => "Game-Mod.vapor.toml",
            Self::ExtensionMod => "Extension-Mod.vapor.toml",
            Self::Crate => "Crate.vapor.toml",
        }
    }

    /// Stable CLI, manifest-section, and API spelling.
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
            Self::Crate => "crate",
        }
    }
}

impl fmt::Display for ContentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ContentKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| format!("unknown Vapor content kind '{value}'"))
    }
}

/// Compatibility name for the canonical content kind.
pub type ContentType = ContentKind;

/// One authored dependency requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySpec {
    /// Referenced content identity.
    pub id: VaporId,
    /// Accepted semantic versions.
    pub version: VersionReq,
    /// Optional edges are left out of automatic graph resolution.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

fn is_false(value: &bool) -> bool {
    !value
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

/// Child kinds a pack may reference before full graph validation.
pub fn allowed_pack_children(parent: ContentType) -> Option<&'static [ContentType]> {
    match parent {
        ContentType::Packagepack => Some(PACKAGEPACK_CHILDREN),
        ContentType::Enginepack => Some(ENGINEPACK_CHILDREN),
        ContentType::Gamepack => Some(GAMEPACK_CHILDREN),
        ContentType::Modpack => Some(MODPACK_CHILDREN),
        _ => None,
    }
}

/// Whether one content kind may directly depend on another.
pub fn dependency_kind_allowed(parent: ContentKind, child: ContentKind) -> bool {
    if parent == ContentKind::Crate {
        return child == ContentKind::Crate;
    }
    match allowed_pack_children(parent) {
        Some(children) => children.contains(&child),
        None if parent == ContentKind::Game => {
            matches!(child, ContentKind::Engine | ContentKind::Crate)
        }
        None => true,
    }
}
