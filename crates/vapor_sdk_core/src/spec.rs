//! Human-readable SDK command specifications for current stub handlers.

mod content;
mod environment;
mod repair;
mod root;
mod steam;
mod template;
mod toolchain;
mod workspace;

use crate::commands::SdkCommand;

/// Broad state surface a future implementation may read or mutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateSurface {
    ReadOnly,
    Environment,
    RepairPlan,
    RepairApply,
    Toolchain,
    AuthoredSource,
    AuthoredComposition,
    BuildArtifact,
    Publication,
    RootRelease,
    Steam,
}

/// Command contract used by placeholder UIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub action: String,
    pub summary: &'static str,
    pub surface: StateSurface,
    pub preconditions: &'static [&'static str],
    pub future_effects: &'static [&'static str],
}

/// Describe an SDK command without executing it.
pub fn describe_command(command: &SdkCommand) -> CommandSpec {
    match command {
        SdkCommand::Version => spec(
            "sdk version",
            "Print SDK version and build identity.",
            StateSurface::ReadOnly,
            &[],
            &["display version metadata"],
        ),
        SdkCommand::Status => spec(
            "sdk status",
            "Summarize SDK health and authoring environment state.",
            StateSurface::ReadOnly,
            &[],
            &["display toolchain, template, and project state"],
        ),
        SdkCommand::Environment(command) => environment::describe(command),
        SdkCommand::Root(command) => root::describe(command),
        SdkCommand::Steam(command) => steam::describe(command),
        SdkCommand::Workspace(command) => workspace::describe(command),
        SdkCommand::Repair(command) => repair::describe(command),
        SdkCommand::Toolchain(command) => toolchain::describe(command),
        SdkCommand::Template(command) => template::describe(command),
        SdkCommand::Packagepack(command) => content::describe_packagepack(command),
        SdkCommand::Pack { pack_type, command } => content::describe_pack(*pack_type, command),
        SdkCommand::Leaf {
            content_type,
            command,
        } => content::describe_leaf(*content_type, command),
    }
}

pub(super) fn read_spec(action: impl Into<String>, summary: &'static str) -> CommandSpec {
    spec(
        action,
        summary,
        StateSurface::ReadOnly,
        &[],
        &["display requested information"],
    )
}

pub(super) fn spec(
    action: impl Into<String>,
    summary: &'static str,
    surface: StateSurface,
    preconditions: &'static [&'static str],
    future_effects: &'static [&'static str],
) -> CommandSpec {
    CommandSpec {
        action: action.into(),
        summary,
        surface,
        preconditions,
        future_effects,
    }
}
