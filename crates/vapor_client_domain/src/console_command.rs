use crate::{
    ClientAction, CompositionAction, WorkspaceAction, WorkspaceProjectSelection,
    WorkspaceToolOperation,
};
use vapor_console_core::{CommandLineError, tokenize_command_line};

/// Capability profile used to build a frontend's available command list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleCommandScope {
    /// A console that may only inspect its current service state.
    LauncherReadOnly,
    /// A console attached to an authoring workspace.
    Workspace,
}

/// One discoverable command entry shared by Vapor frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleCommandSpec {
    /// Canonical command path, without arguments.
    pub path: &'static str,
    /// Short description shown by `help` and command suggestions.
    pub summary: &'static str,
    /// Whether the command can mutate state or start work.
    pub mutating: bool,
    workspace_only: bool,
}

const COMMANDS: &[ConsoleCommandSpec] = &[
    ConsoleCommandSpec {
        path: "help",
        summary: "Show available commands",
        mutating: false,
        workspace_only: false,
    },
    ConsoleCommandSpec {
        path: "status",
        summary: "Show the current Vapor service status",
        mutating: false,
        workspace_only: false,
    },
    ConsoleCommandSpec {
        path: "catalog refresh",
        summary: "Refresh discovered content",
        mutating: true,
        workspace_only: false,
    },
    ConsoleCommandSpec {
        path: "composition select <id>",
        summary: "Select a saved composition",
        mutating: true,
        workspace_only: false,
    },
    ConsoleCommandSpec {
        path: "composition launch <id>",
        summary: "Launch an installed composition",
        mutating: true,
        workspace_only: false,
    },
    ConsoleCommandSpec {
        path: "workspace format|check|test|build|validate",
        summary: "Run a managed workspace tool",
        mutating: true,
        workspace_only: true,
    },
];

/// Commands accepted by the interactive Vapor console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommand {
    Help,
    Status,
    CatalogRefresh,
    CompositionSelect(String),
    CompositionLaunch(String),
    WorkspaceTool(WorkspaceToolOperation),
}

/// Parse failures shown inline in a console rather than as host errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommandError {
    Empty,
    Unknown(String),
    MissingArgument(&'static str),
    UnexpectedArgument,
    MalformedLine(CommandLineError),
}

/// A parsed command's immediate effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommandOutcome {
    Help,
    Status,
    Action(ClientAction),
}

impl ConsoleCommand {
    /// Parse one user-entered line into a shared, typed service intent.
    pub fn parse(line: &str) -> Result<Self, ConsoleCommandError> {
        let arguments = tokenize_command_line(line)
            .map_err(ConsoleCommandError::MalformedLine)?;
        let Some((command, rest)) = arguments.split_first() else {
            return Err(ConsoleCommandError::Empty);
        };
        match (command.as_str(), rest) {
            ("help", []) => Ok(Self::Help),
            ("status", []) => Ok(Self::Status),
            ("catalog", [subcommand]) if subcommand == "refresh" => Ok(Self::CatalogRefresh),
            ("composition", [subcommand, id]) if subcommand == "select" => {
                Ok(Self::CompositionSelect(id.clone()))
            }
            ("composition", [subcommand, id]) if subcommand == "launch" => {
                Ok(Self::CompositionLaunch(id.clone()))
            }
            ("workspace", [subcommand]) => {
                let operation = match subcommand.as_str() {
                    "format" | "fmt" => WorkspaceToolOperation::Format,
                    "check" => WorkspaceToolOperation::Check,
                    "test" => WorkspaceToolOperation::Test,
                    "build" => WorkspaceToolOperation::Build,
                    "validate" => WorkspaceToolOperation::Validate,
                    _ => return Err(ConsoleCommandError::Unknown(line.trim().to_owned())),
                };
                Ok(Self::WorkspaceTool(operation))
            }
            ("composition", [subcommand]) if subcommand == "select" || subcommand == "launch" => {
                Err(ConsoleCommandError::MissingArgument("composition id"))
            }
            ("catalog", [subcommand]) if subcommand == "refresh" => {
                Err(ConsoleCommandError::UnexpectedArgument)
            }
            _ => Err(ConsoleCommandError::Unknown(line.trim().to_owned())),
        }
    }

    /// Convert a command to the existing typed Client service action.
    pub fn outcome(&self) -> ConsoleCommandOutcome {
        match self {
            Self::Help => ConsoleCommandOutcome::Help,
            Self::Status => ConsoleCommandOutcome::Status,
            Self::CatalogRefresh => ConsoleCommandOutcome::Action(ClientAction::Catalog(
                crate::CatalogAction::Refresh,
            )),
            Self::CompositionSelect(composition_id) => ConsoleCommandOutcome::Action(
                ClientAction::Composition(CompositionAction::Select {
                    composition_id: composition_id.clone(),
                }),
            ),
            Self::CompositionLaunch(composition_id) => ConsoleCommandOutcome::Action(
                ClientAction::Composition(CompositionAction::Launch {
                    composition_id: composition_id.clone(),
                }),
            ),
            Self::WorkspaceTool(operation) => ConsoleCommandOutcome::Action(
                ClientAction::Workspace(WorkspaceAction::RunTool {
                    operation: *operation,
                    project_selection: WorkspaceProjectSelection::All,
                    target_triples: Vec::new(),
                }),
            ),
        }
    }
}

/// Return the command descriptors allowed for a frontend scope.
pub fn console_command_catalog(scope: ConsoleCommandScope) -> Vec<ConsoleCommandSpec> {
    COMMANDS
        .iter()
        .copied()
        .filter(|command| {
            scope == ConsoleCommandScope::Workspace || !command.workspace_only && !command.mutating
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientAction, WorkspaceAction, WorkspaceToolOperation};

    #[test]
    fn console_commands_parse_into_the_shared_typed_client_actions() {
        assert_eq!(
            ConsoleCommand::parse("workspace build").unwrap().outcome(),
            ConsoleCommandOutcome::Action(ClientAction::Workspace(WorkspaceAction::RunTool {
                operation: WorkspaceToolOperation::Build,
                project_selection: Default::default(),
                target_triples: Vec::new(),
            }))
        );
        assert_eq!(
            ConsoleCommand::parse("composition select \"Loo Cast\"")
                .unwrap()
                .outcome(),
            ConsoleCommandOutcome::Action(ClientAction::Composition(
                crate::CompositionAction::Select {
                    composition_id: "Loo Cast".to_owned(),
                }
            ))
        );
    }

    #[test]
    fn console_command_catalog_respects_launcher_read_only_scope() {
        let launcher = console_command_catalog(ConsoleCommandScope::LauncherReadOnly);
        assert!(launcher.iter().all(|command| !command.mutating));
        assert!(!launcher.iter().any(|command| command.path == "workspace build"));
    }

    #[test]
    fn malformed_command_quotes_produce_a_user_facing_parse_error() {
        assert!(ConsoleCommand::parse("composition select \"Loo Cast").is_err());
    }
}
