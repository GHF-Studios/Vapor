//! Vapor content identity vocabulary shared by tools and runtimes.

use std::fmt;
use std::str::FromStr;

/// Places Vapor tooling can list, discover, or resolve content from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSource {
    /// Already discovered through configured sources.
    Discovered,
    /// Local filesystem or local user library.
    Local,
    /// Git-backed sources.
    Git,
    /// Steam Workshop-backed sources.
    Workshop,
    /// Every configured source.
    All,
}

impl ContentSource {
    /// Stable CLI and manifest spelling for this content source.
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
    type Err = ParseContentValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "discovered" => Ok(Self::Discovered),
            "local" => Ok(Self::Local),
            "git" => Ok(Self::Git),
            "workshop" => Ok(Self::Workshop),
            "all" => Ok(Self::All),
            _ => Err(ParseContentValueError::new("content source", value)),
        }
    }
}

/// Vapor content kind used by manifests, tooling, composition, and launch workflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
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

impl ContentType {
    /// Stable CLI and manifest spelling for this content type.
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
        }
    }

    /// Whether this content type can contain child content.
    pub const fn is_pack(self) -> bool {
        matches!(
            self,
            Self::Packagepack | Self::Enginepack | Self::Gamepack | Self::Modpack
        )
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ContentType {
    type Err = ParseContentValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "packagepack" => Ok(Self::Packagepack),
            "enginepack" => Ok(Self::Enginepack),
            "gamepack" => Ok(Self::Gamepack),
            "modpack" => Ok(Self::Modpack),
            "engine" => Ok(Self::Engine),
            "game" => Ok(Self::Game),
            "engine-mod" => Ok(Self::EngineMod),
            "game-mod" => Ok(Self::GameMod),
            "extension-mod" => Ok(Self::ExtensionMod),
            _ => Err(ParseContentValueError::new("content type", value)),
        }
    }
}

/// Error returned when a stable Vapor content spelling cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseContentValueError {
    kind: &'static str,
    value: String,
}

impl ParseContentValueError {
    fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

impl fmt::Display for ParseContentValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown {} `{}`", self.kind, self.value)
    }
}

impl std::error::Error for ParseContentValueError {}

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

/// Child content types a parent pack may contain before deeper graph validation runs.
pub fn allowed_pack_children(parent: ContentType) -> Option<&'static [ContentType]> {
    match parent {
        ContentType::Packagepack => Some(PACKAGEPACK_CHILDREN),
        ContentType::Enginepack => Some(ENGINEPACK_CHILDREN),
        ContentType::Gamepack => Some(GAMEPACK_CHILDREN),
        ContentType::Modpack => Some(MODPACK_CHILDREN),
        _ => None,
    }
}
