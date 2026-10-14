//! SDK template command specifications.

use super::{CommandSpec, read_spec};
use crate::template::TemplateCommand;

pub(super) fn describe(command: &TemplateCommand) -> CommandSpec {
    match command {
        TemplateCommand::List => {
            read_spec("sdk template list", "List available authoring templates.")
        }
        TemplateCommand::Info => read_spec(
            "sdk template info",
            "Inspect template metadata and intended use.",
        ),
    }
}
