//! Shared parser for role-specific, source-authored Vapor content manifests.
//!
//! This module owns the meaning and shape of authored declarations. Filesystem
//! discovery, source trust, Workshop acquisition, and installation remain host
//! responsibilities.

use crate::{ContentKind, VaporId};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// Parsed role-specific content manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthoredContentManifest {
    /// Deployed-artifact schema marker; absent in source-authored manifests.
    pub schema: Option<u32>,
    engine: Option<AuthoredContent>,
    game: Option<AuthoredContent>,
    packagepack: Option<AuthoredContent>,
    enginepack: Option<AuthoredContent>,
    gamepack: Option<AuthoredContent>,
    modpack: Option<AuthoredContent>,
    #[serde(rename = "engine-mod")]
    engine_mod: Option<AuthoredContent>,
    #[serde(rename = "game-mod")]
    game_mod: Option<AuthoredContent>,
    #[serde(rename = "extension-mod")]
    extension_mod: Option<AuthoredContent>,
    #[serde(rename = "crate")]
    crate_content: Option<AuthoredContent>,
}

impl AuthoredContentManifest {
    /// Parse a source-authored role manifest.
    pub fn parse(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    /// Return the declaration for the expected role, if present.
    pub fn content(&self, kind: ContentKind) -> Option<&AuthoredContent> {
        match kind {
            ContentKind::Engine => self.engine.as_ref(),
            ContentKind::Game => self.game.as_ref(),
            ContentKind::Packagepack => self.packagepack.as_ref(),
            ContentKind::Enginepack => self.enginepack.as_ref(),
            ContentKind::Gamepack => self.gamepack.as_ref(),
            ContentKind::Modpack => self.modpack.as_ref(),
            ContentKind::EngineMod => self.engine_mod.as_ref(),
            ContentKind::GameMod => self.game_mod.as_ref(),
            ContentKind::ExtensionMod => self.extension_mod.as_ref(),
            ContentKind::Crate => self.crate_content.as_ref(),
        }
    }

    /// Number of role sections declared in this manifest.
    pub fn content_count(&self) -> usize {
        ContentKind::ALL
            .into_iter()
            .filter(|kind| self.content(*kind).is_some())
            .count()
    }

    /// Deployed-artifact schema marker, when present.
    pub fn schema_version(&self) -> Option<u32> {
        self.schema
    }
}

/// Authored declaration shared by content discovery and graph resolution.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuthoredContent {
    /// Present in a deployed artifact; source identities are inferred from the
    /// checked workspace layout instead.
    pub id: Option<VaporId>,
    /// Present in a deployed artifact; source display names come from the
    /// workspace declaration.
    pub name: Option<String>,
    #[serde(default)]
    pub version: AuthoredVersion,
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default)]
    pub libraries: Vec<String>,
    #[serde(default)]
    pub runtime: Vec<AuthoredRuntimePayload>,
    #[serde(default, rename = "steam")]
    pub workshop: AuthoredWorkshopPolicy,
    #[serde(default)]
    pub dependencies: Vec<AuthoredReference>,
    #[serde(default)]
    pub conflicts: Vec<AuthoredReference>,
    pub engine: Option<AuthoredReference>,
    pub enginepack: Option<AuthoredReference>,
    pub game: Option<AuthoredReference>,
    pub gamepack: Option<AuthoredReference>,
}

/// Runtime files declared by a deployed artifact.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuthoredRuntimePayload {
    /// Runtime target label.
    pub target: String,
    /// Executable names included for this target.
    #[serde(default)]
    pub binaries: Vec<String>,
    /// Shared-library names included for this target.
    #[serde(default)]
    pub libraries: Vec<String>,
}

impl AuthoredContent {
    /// Return declared dependencies together with the role of each edge.
    ///
    /// Explicit dependencies retain their declaration order, followed by
    /// composition edges implied by the content kind.
    pub fn dependency_edges(&self, kind: ContentKind) -> Vec<AuthoredContentEdge> {
        let mut edges: Vec<_> = self
            .dependencies
            .iter()
            .cloned()
            .map(|reference| AuthoredContentEdge {
                relationship: reference.relationship.clone(),
                reference,
            })
            .collect();
        match kind {
            ContentKind::Packagepack => {
                push_role_edge(&mut edges, "engine", self.engine.as_ref());
                push_role_edge(&mut edges, "enginepack", self.enginepack.as_ref());
                push_role_edge(&mut edges, "game", self.game.as_ref());
                push_role_edge(&mut edges, "gamepack", self.gamepack.as_ref());
            }
            ContentKind::Game | ContentKind::Enginepack => {
                push_role_edge(&mut edges, "engine", self.engine.as_ref());
            }
            ContentKind::Gamepack => push_role_edge(&mut edges, "game", self.game.as_ref()),
            _ => {}
        }
        edges
    }
}

fn push_role_edge(
    edges: &mut Vec<AuthoredContentEdge>,
    relationship: &str,
    reference: Option<&AuthoredReference>,
) {
    if let Some(reference) = reference {
        edges.push(AuthoredContentEdge {
            relationship: relationship.to_owned(),
            reference: reference.clone(),
        });
    }
}

/// One dependency edge with its semantic role.
#[derive(Debug, Clone)]
pub struct AuthoredContentEdge {
    /// Role assigned to this graph edge.
    pub relationship: String,
    /// Authored reference data.
    pub reference: AuthoredReference,
}

/// Authored version declaration, including workspace-inherited versions.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
pub enum AuthoredVersion {
    /// Exact version string.
    Exact(Version),
    /// Inherit a version from the workspace when true.
    Workspace { workspace: bool },
    /// No version declaration.
    #[default]
    Missing,
}

impl AuthoredVersion {
    /// Resolve this declaration against the enclosing workspace version.
    pub fn resolve(&self, workspace_version: Option<&Version>) -> Option<Version> {
        match self {
            Self::Exact(version) => Some(version.clone()),
            Self::Workspace { workspace: true } => workspace_version.cloned(),
            Self::Workspace { workspace: false } | Self::Missing => None,
        }
    }
}

/// Authored dependency, conflict, or role edge.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuthoredReference {
    /// Referenced content ID.
    pub id: VaporId,
    /// Authored relationship label.
    #[serde(default = "default_relationship")]
    pub relationship: String,
    /// Whether this reference is optional.
    #[serde(default)]
    pub optional: bool,
    /// Version requirement.
    pub version: Option<VersionReq>,
    /// Workshop item ID used to acquire this content.
    pub workshop_id: Option<String>,
    /// Author explanation for this reference.
    pub reason: Option<String>,
}

fn default_relationship() -> String {
    "dependency".to_owned()
}

/// Authored Workshop metadata on a content declaration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuthoredWorkshopPolicy {
    /// Steam application ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<u32>,
    /// Existing Workshop item ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_file_id: Option<String>,
    /// Visibility setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    /// Workshop title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workshop description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Workshop tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Default change note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_note: Option<String>,
}

impl AuthoredWorkshopPolicy {
    /// Steam AppID targeted by this Workshop item.
    pub fn app_id(&self) -> Option<u32> {
        self.app_id
    }

    /// PublishedFileId for an existing Workshop item.
    pub fn published_file_id(&self) -> Option<&str> {
        self.published_file_id.as_deref()
    }

    /// Authored visibility string.
    pub fn visibility(&self) -> Option<&str> {
        self.visibility.as_deref()
    }

    /// Authored Workshop title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Authored Workshop description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Authored Workshop tags.
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Default update note.
    pub fn change_note(&self) -> Option<&str> {
        self.change_note.as_deref()
    }

    /// Whether this policy contains no publication metadata.
    pub fn is_empty(&self) -> bool {
        self.app_id.is_none()
            && self.published_file_id.is_none()
            && self.visibility.is_none()
            && self.title.is_none()
            && self.description.is_none()
            && self.tags.is_empty()
            && self.change_note.is_none()
    }
}
