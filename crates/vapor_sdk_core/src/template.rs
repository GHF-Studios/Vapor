//! SDK template command intent.

/// Template discovery commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateCommand {
    /// List available project/content templates.
    List,
    /// Inspect template metadata and intended use.
    Info,
}
