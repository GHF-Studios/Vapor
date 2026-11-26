pub use vapor_client::{ClientHostArguments, ClientHostContext, HostPlatform};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, ffi::OsString, path::PathBuf};

    fn env(entries: &[(&str, &str)]) -> HashMap<OsString, OsString> {
        entries
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    #[test]
    fn explicit_arguments_override_environment() {
        let context = ClientHostContext::resolve(
            ClientHostArguments {
                application_executable: PathBuf::from("/install/bin/vapor-gui"),
                workspace_root: Some(PathBuf::from("/explicit/workspace")),
                state_directory: Some(PathBuf::from("/explicit/state")),
            },
            &env(&[
                ("VAPOR_STUDIO_WORKSPACE", "/environment/workspace"),
                ("VAPOR_STUDIO_STATE_DIR", "/environment/state"),
                ("XDG_DATA_HOME", "/environment/data"),
            ]),
            HostPlatform::Linux,
        )
        .expect("host context resolves");

        assert_eq!(
            context.workspace_root,
            Some(PathBuf::from("/explicit/workspace"))
        );
        assert_eq!(context.state_directory, PathBuf::from("/explicit/state"));
    }

    #[test]
    fn vapor_home_owns_the_default_client_state_directory() {
        let context = ClientHostContext::resolve(
            ClientHostArguments {
                application_executable: PathBuf::from("/install/bin/vapor-gui"),
                workspace_root: None,
                state_directory: None,
            },
            &env(&[
                ("HOME", "/home/player"),
                ("XDG_DATA_HOME", "/home/player/.local/share"),
                ("VAPOR_HOME", "/home/player/Vapor"),
            ]),
            HostPlatform::Linux,
        )
        .expect("host context resolves without optional values");

        assert_eq!(context.workspace_root, None);
        assert_eq!(
            context.state_directory,
            PathBuf::from("/home/player/Vapor/data/state/studio")
        );
    }

    #[test]
    fn secrets_in_inherited_environment_are_never_retained_or_formatted() {
        let context = ClientHostContext::resolve(
            ClientHostArguments {
                application_executable: PathBuf::from("/install/bin/vapor-gui"),
                workspace_root: None,
                state_directory: None,
            },
            &env(&[
                ("HOME", "/home/player"),
                ("VAPOR_HOME", "/home/player/Vapor"),
                ("STEAM_PASSWORD", "never-print-this-secret"),
                ("VAPOR_STEAM_TOKEN", "also-secret"),
            ]),
            HostPlatform::Linux,
        )
        .expect("host context resolves");

        let diagnostic = format!("{context:?}");
        assert!(!diagnostic.contains("never-print-this-secret"));
        assert!(!diagnostic.contains("also-secret"));
    }
}
