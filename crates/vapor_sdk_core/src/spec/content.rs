//! SDK content command specifications.

use super::{CommandSpec, StateSurface, read_spec, spec};
use crate::commands::{
    ContentReadCommand, LeafCommand, PackCommand, PackCompositionCommand, PackagepackCommand,
    SourceAuthoringCommand,
};
use crate::content::ContentType;

pub(super) fn describe_packagepack(command: &PackagepackCommand) -> CommandSpec {
    match command {
        PackagepackCommand::Read(command) => describe_read(ContentType::Packagepack, command),
        PackagepackCommand::Author(command) => describe_author(ContentType::Packagepack, command),
        PackagepackCommand::Compose(command) => {
            describe_composition(ContentType::Packagepack, command)
        }
        PackagepackCommand::Lock { .. } => composition_spec(
            "sdk packagepack lock",
            "Write a persistent packagepack lock artifact from the resolved authored graph.",
        ),
    }
}

pub(super) fn describe_pack(pack_type: ContentType, command: &PackCommand) -> CommandSpec {
    match command {
        PackCommand::Read(command) => describe_read(pack_type, command),
        PackCommand::Author(command) => describe_author(pack_type, command),
        PackCommand::Compose(command) => describe_composition(pack_type, command),
    }
}

pub(super) fn describe_leaf(content_type: ContentType, command: &LeafCommand) -> CommandSpec {
    match command {
        LeafCommand::Read(command) => describe_read(content_type, command),
        LeafCommand::Author(command) => describe_author(content_type, command),
    }
}

fn describe_read(content_type: ContentType, command: &ContentReadCommand) -> CommandSpec {
    let action = match command {
        ContentReadCommand::List { .. } => "list",
        ContentReadCommand::Status { .. } => "status",
        ContentReadCommand::Fingerprint { .. } => "fingerprint",
        ContentReadCommand::Inspect { .. } => "inspect",
        ContentReadCommand::Validate { .. } => "validate",
    };
    read_spec(
        format!("sdk {} {action}", content_type.as_str()),
        "Read SDK-known authored or discoverable content state.",
    )
}

fn describe_author(content_type: ContentType, command: &SourceAuthoringCommand) -> CommandSpec {
    let (action, summary, surface) = match command {
        SourceAuthoringCommand::New { .. } => (
            "new",
            "Create a new source project.",
            StateSurface::AuthoredSource,
        ),
        SourceAuthoringCommand::Init { .. } => (
            "init",
            "Initialize the current empty directory as a source project.",
            StateSurface::AuthoredSource,
        ),
        SourceAuthoringCommand::Build { .. } => (
            "build",
            "Build source artifacts.",
            StateSurface::BuildArtifact,
        ),
        SourceAuthoringCommand::Package { .. } => (
            "package",
            "Package content for distribution.",
            StateSurface::BuildArtifact,
        ),
        SourceAuthoringCommand::Publish { .. } => (
            "publish",
            "Publish content through configured release channels.",
            StateSurface::Publication,
        ),
    };
    spec(
        format!("sdk {} {action}", content_type.as_str()),
        summary,
        surface,
        &["content identity is valid for this command"],
        &["perform the authored-content workflow when implemented"],
    )
}

fn describe_composition(pack_type: ContentType, command: &PackCompositionCommand) -> CommandSpec {
    let action = match command {
        PackCompositionCommand::Add { .. } => "add",
        PackCompositionCommand::Remove { .. } => "remove",
        PackCompositionCommand::Select { .. } => "select",
        PackCompositionCommand::Unselect { .. } => "unselect",
    };
    composition_spec(
        format!("sdk {} {action}", pack_type.as_str()),
        "Mutate child membership or active child selection inside an authored pack.",
    )
}

fn composition_spec(action: impl Into<String>, summary: &'static str) -> CommandSpec {
    spec(
        action,
        summary,
        StateSurface::AuthoredComposition,
        &[
            "target pack source is available",
            "child type is allowed by the parent pack type",
        ],
        &["update authored pack membership or active selection when implemented"],
    )
}
