//! Declarative Vapor CLI grammar.
//!
//! Some modeled leaves intentionally exist before their implementation.
//! The command tree is the product model; handlers may arrive incrementally.

#![allow(dead_code)]

use crate::{VaporId, VaporRole};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "vapor",
    version,
    about = "Universal command-line interface for Vapor",
    arg_required_else_help = true
)]
pub(super) struct VaporCli {
    #[command(subcommand)]
    pub(super) command: VaporCommand,
}

#[derive(Debug, Parser)]
#[command(
    name = "vapor-installer",
    version,
    about = "Vapor Installer command-line interface",
    arg_required_else_help = true
)]
pub(super) struct InstallerCli {
    #[command(subcommand)]
    pub(super) command: InstallerCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum VaporCommand {
    Installation {
        #[command(subcommand)]
        command: InstallationCommand,
    },

    Role {
        #[command(subcommand)]
        command: RoleCommand,
    },

    Authority {
        #[command(subcommand)]
        command: AuthorityCommand,
    },

    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommand,
    },

    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },

    Ecosystem {
        #[command(subcommand)]
        command: EcosystemCommand,
    },

    Packagepack {
        #[command(subcommand)]
        command: PackagepackCommand,
    },

    Enginepack {
        #[command(subcommand)]
        command: GraphContentCommand,
    },

    Gamepack {
        #[command(subcommand)]
        command: GraphContentCommand,
    },

    Modpack {
        #[command(subcommand)]
        command: GraphContentCommand,
    },

    Engine {
        #[command(subcommand)]
        command: BehavioralContentCommand,
    },

    Game {
        #[command(subcommand)]
        command: BehavioralContentCommand,
    },

    EngineMod {
        #[command(subcommand)]
        command: BehavioralContentCommand,
    },

    GameMod {
        #[command(subcommand)]
        command: BehavioralContentCommand,
    },

    ExtensionMod {
        #[command(subcommand)]
        command: BehavioralContentCommand,
    },

    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum InstallerCommand {
    Installation {
        #[command(subcommand)]
        command: InstallationCommand,
    },

    Role {
        #[command(subcommand)]
        command: RoleCommand,
    },

    Authority {
        #[command(subcommand)]
        command: AuthorityCommand,
    },

    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum InstallationCommand {
    Status,
    Diagnose,
    Repair,
}

#[derive(Debug, Subcommand)]
pub(super) enum RoleCommand {
    Status,

    Promote {
        #[arg(value_name = "ROLE")]
        role: VaporRole,
    },

    Demote {
        #[arg(value_name = "ROLE")]
        role: VaporRole,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum AuthorityCommand {
    Status,
}

#[derive(Debug, Subcommand)]
pub(super) enum ToolchainCommand {
    Status,
    Install,
    Diagnose,
    Repair,

    /// Run Cargo from the active Vapor Installation's managed Rust toolchain.
    Cargo {
        /// Explicit Vapor Project to use as Cargo's execution context.
        ///
        /// Usually unnecessary when Cargo's `-p/--package` identifies a
        /// unique Project or the current directory lies within one.
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,

        /// Arguments forwarded verbatim to Cargo after `--`.
        #[arg(last = true, value_name = "ARG")]
        args: Vec<std::ffi::OsString>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum SourceCommand {
    Status,
    List,

    Acquire {
        #[arg(value_name = "SOURCE")]
        source: String,
    },

    Fork {
        #[arg(value_name = "SOURCE")]
        source: String,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum EcosystemCommand {
    Status,

    Acquire {
        #[arg(value_name = "SOURCE")]
        source: Option<String>,
    },

    Fork {
        #[arg(value_name = "SOURCE")]
        source: Option<String>,
    },

    Create {
        #[arg(value_name = "IDENTITY")]
        identity: Option<String>,
    },

    Build,
    Test,
    Publish,

    Deploy {
        #[command(subcommand)]
        command: EcosystemDeployCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum EcosystemDeployCommand {
    /// Deploy into the active local Vapor App Instance.
    Local,

    /// Deploy through SteamPipe.
    Steam(SteamDeployArgs),
}

#[derive(Debug, Args)]
pub(super) struct SteamDeployArgs {
    /// Perform a SteamPipe preview build.
    #[arg(long)]
    pub(super) preview: bool,

    /// Steam build account. An explicit account may be remembered locally.
    #[arg(long, value_name = "ACCOUNT")]
    pub(super) account: Option<String>,

    /// Explicit SteamCMD path override.
    #[arg(long, value_name = "PATH")]
    pub(super) steamcmd: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(super) enum PackagepackCommand {
    Create(CreateContentArgs),
    List(ContentListArgs),
    Inspect(LocalContentTargetArgs),
    Resolve(LocalContentTargetArgs),
    Verify(LocalContentTargetArgs),
    Build(LocalContentTargetArgs),
    Test(LocalContentTargetArgs),
    Install(ContentIdentityArgs),
    Select(ContentIdentityArgs),
    Run(LocalContentTargetArgs),
    Remove(ContentIdentityArgs),
    Publish(ContentIdentityArgs),
}

#[derive(Debug, Subcommand)]
pub(super) enum GraphContentCommand {
    Create(CreateContentArgs),
    List(ContentListArgs),
    Inspect(LocalContentTargetArgs),
    Resolve(LocalContentTargetArgs),
    Verify(LocalContentTargetArgs),
    Test(LocalContentTargetArgs),
    Publish(ContentIdentityArgs),
}

#[derive(Debug, Subcommand)]
pub(super) enum BehavioralContentCommand {
    Create(CreateContentArgs),
    List(ContentListArgs),
    Inspect(LocalContentTargetArgs),
    Verify(LocalContentTargetArgs),
    Test(LocalContentTargetArgs),
    Publish(ContentIdentityArgs),
}

#[derive(Debug, Subcommand)]
pub(super) enum LibraryCommand {
    Create(CreateContentArgs),
    List(ContentListArgs),
    Inspect(LocalContentTargetArgs),
    Resolve(LocalContentTargetArgs),
    Verify(LocalContentTargetArgs),
    Repair(LocalContentTargetArgs),
    Test(LocalContentTargetArgs),
    Publish(ContentIdentityArgs),
}

#[derive(Debug, Args)]
pub(super) struct CreateContentArgs {
    #[arg(value_name = "ID")]
    pub(super) id: Option<VaporId>,

    #[arg(long, value_name = "TEMPLATE")]
    pub(super) template: Option<String>,

    #[arg(long, value_name = "PATH")]
    pub(super) root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct ContentListArgs {
    #[arg(long, value_name = "PATH")]
    pub(super) root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct LocalContentTargetArgs {
    #[arg(value_name = "ID")]
    pub(super) id: Option<VaporId>,

    /// Explicit local source/catalog root.
    ///
    /// Normally Vapor should infer source context. This remains available as
    /// an explicit bootstrap/advanced override.
    #[arg(long, value_name = "PATH")]
    pub(super) root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct ContentIdentityArgs {
    #[arg(value_name = "ID")]
    pub(super) id: Option<VaporId>,
}
