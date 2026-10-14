//! Typed SDK command requests.

use crate::content::{ContentSource, ContentType};
use crate::environment::EnvironmentCommand;
use crate::repair::RepairCommand;
use crate::root::types::RootCommand;
use crate::steam::types::SteamCommand;
use crate::template::TemplateCommand;
use crate::toolchain::ToolchainCommand;
use crate::workspace::WorkspaceCommand;
use vapor_core::ChildContentRef;

/// Read-only commands shared by every SDK content kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentReadCommand {
    List { source: ContentSource },
    Status { content_id: String },
    Fingerprint { content_id: String },
    Inspect { content_id: String },
    Validate { content_id: String },
}

/// Source authoring commands shared by every SDK content kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceAuthoringCommand {
    New { content_id: String },
    Init { content_id: String },
    Build { content_id: String },
    Package { content_id: String },
    Publish { content_id: String },
}

/// Composition mutations for authored packs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackCompositionCommand {
    Add {
        pack_id: String,
        child: ChildContentRef,
    },
    Remove {
        pack_id: String,
        child: ChildContentRef,
    },
    Select {
        pack_id: String,
        child: ChildContentRef,
    },
    Unselect {
        pack_id: String,
        child: ChildContentRef,
    },
}

/// Packagepack authoring commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackagepackCommand {
    Read(ContentReadCommand),
    Author(SourceAuthoringCommand),
    Compose(PackCompositionCommand),
    /// Write a persistent packagepack lock artifact from the resolved graph.
    Lock {
        packagepack_id: String,
    },
}

/// Commands shared by authored non-root pack types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackCommand {
    Read(ContentReadCommand),
    Author(SourceAuthoringCommand),
    Compose(PackCompositionCommand),
}

/// Commands shared by authored leaf content types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafCommand {
    Read(ContentReadCommand),
    Author(SourceAuthoringCommand),
}

/// Root SDK workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkCommand {
    Version,
    Status,
    Environment(EnvironmentCommand),
    Root(RootCommand),
    Steam(SteamCommand),
    Workspace(WorkspaceCommand),
    Repair(RepairCommand),
    Toolchain(ToolchainCommand),
    Template(TemplateCommand),
    Packagepack(PackagepackCommand),
    Pack {
        pack_type: ContentType,
        command: PackCommand,
    },
    Leaf {
        content_type: ContentType,
        command: LeafCommand,
    },
}
