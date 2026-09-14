use super::xml_escape;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn build(
    project_root: &Path,
    installation_root: &Path,
) -> Vec<(PathBuf, String)> {
    #[cfg(windows)]
    {
        let _ = (project_root, installation_root);
        return Vec::new();
    }

    #[cfg(not(windows))]
    {
        let run_root = project_root.join(".run");
        let vapor = installation_root.join("bin/vapor");

        let mut configurations = [
            ("Vapor_Installation_Diagnose.run.xml", "Vapor · Installation · Diagnose", "installation diagnose"),
            ("Vapor_Installation_Repair.run.xml", "Vapor · Installation · Repair", "installation repair"),
            ("Vapor_Client_Status.run.xml", "Vapor · Client · Status", "client status"),
            ("Vapor_Client_Build.run.xml", "Vapor · Client · Build", "client build"),
            ("Vapor_Client_Test.run.xml", "Vapor · Client · Test", "client test"),
            ("Vapor_Client_Deploy_Local.run.xml", "Vapor · Client · Deploy Local", "client deploy local"),
            ("Vapor_Platform_Server_Status.run.xml", "Vapor · Platform Server · Status", "platform-server status"),
            ("Vapor_Platform_Server_Runs.run.xml", "Vapor · Platform Server · Runs", "platform-server runs"),
            ("Vapor_Platform_Server_Build.run.xml", "Vapor · Platform Server · Build", "platform-server build"),
            ("Vapor_Platform_Server_Test.run.xml", "Vapor · Platform Server · Test", "platform-server test"),
            ("Vapor_Platform_Server_Deploy.run.xml", "Vapor · Platform Server · Deploy", "platform-server deploy"),
        ]
        .into_iter()
        .map(|(filename, name, arguments)| {
            (
                run_root.join(filename),
                shell_configuration(name, &vapor, arguments),
            )
        })
        .collect::<Vec<_>>();

        configurations.extend(client_run_configurations(project_root, &run_root, &vapor));
        configurations
    }
}

#[cfg(not(windows))]
#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[cfg(not(windows))]
#[derive(Deserialize)]
struct CargoPackage {
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}

#[cfg(not(windows))]
#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[cfg(not(windows))]
fn client_run_configurations(
    project_root: &Path,
    run_root: &Path,
    vapor: &Path,
) -> Vec<(PathBuf, String)> {
    let Some(app_root) = child_named(project_root, "Vapor-Client") else {
        return Vec::new();
    };
    let Ok(source) = fs::read_to_string(app_root.join("App.vapor.toml")) else {
        return Vec::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(&source) else {
        return Vec::new();
    };
    let Some(content) = manifest
        .get("root")
        .and_then(|root| root.get("content"))
        .and_then(toml::Value::as_array)
    else {
        return Vec::new();
    };

    let mut configurations = Vec::new();

    for artifact in content.iter().filter_map(toml::Value::as_table) {
        if artifact.get("kind").and_then(toml::Value::as_str) != Some("packagepack")
            || artifact
                .get("default-launch")
                .and_then(toml::Value::as_str)
                .is_none()
        {
            continue;
        }

        let Some(id) = artifact.get("id").and_then(toml::Value::as_str) else {
            continue;
        };
        let parts = id.split('/').collect::<Vec<_>>();
        if parts.len() < 3 {
            continue;
        }

        let repository_id = parts[parts.len() - 2];
        let target_id = parts[parts.len() - 1]
            .strip_suffix("-packagepack")
            .unwrap_or(repository_id);
        let Some(repository_root) = child_named(project_root, repository_id) else {
            continue;
        };

        let binaries = cargo_binaries(vapor, &repository_root);
        let multiple = binaries.len() > 1;

        for (package_root, binary) in binaries {
            let label = if multiple {
                format!("{} · {}", title(target_id), title(&binary))
            } else {
                title(target_id)
            };
            let suffix = if multiple {
                format!("{target_id}-{binary}")
            } else {
                target_id.to_owned()
            };
            let arguments = format!(
                "toolchain cargo -- run --manifest-path Cargo.toml --bin {}",
                shell_quote(&binary),
            );

            configurations.push((
                run_root.join(format!("Vapor_Client_Run_{}.run.xml", file_id(&suffix))),
                shell_configuration_at(
                    &format!("Vapor · Client · Run · {label}"),
                    vapor,
                    &arguments,
                    &project_path(project_root, &package_root),
                ),
            ));
        }
    }

    configurations
}

#[cfg(not(windows))]
fn cargo_binaries(vapor: &Path, root: &Path) -> Vec<(PathBuf, String)> {
    let manifest = root.join("Cargo.toml");
    let Ok(output) = Command::new(vapor)
        .args([
            "toolchain", "cargo", "--", "metadata", "--format-version", "1", "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest)
        .current_dir(root)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let Ok(metadata) = serde_json::from_slice::<CargoMetadata>(&output.stdout) else {
        return Vec::new();
    };
    let mut binaries = metadata
        .packages
        .into_iter()
        .flat_map(|package| {
            let root = package.manifest_path.parent().map(Path::to_path_buf);
            package
                .targets
                .into_iter()
                .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
                .filter_map(move |target| root.clone().map(|root| (root, target.name)))
        })
        .collect::<Vec<_>>();
    binaries.sort();
    binaries.dedup();
    binaries
}

#[cfg(not(windows))]
fn child_named(root: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
}

#[cfg(not(windows))]
fn project_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .map(|path| format!("$PROJECT_DIR$/{path}"))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
fn title(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| format!("{}{}", first.to_uppercase(), chars.as_str()))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(not(windows))]
fn file_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(not(windows))]
fn shell_configuration(name: &str, vapor: &Path, arguments: &str) -> String {
    shell_configuration_at(name, vapor, arguments, "$PROJECT_DIR$")
}

#[cfg(not(windows))]
fn shell_configuration_at(
    name: &str,
    vapor: &Path,
    arguments: &str,
    working_directory: &str,
) -> String {
    let command = format!("{} {arguments}", shell_quote(&vapor.to_string_lossy()));

    format!(
        "<component name=\"ProjectRunConfigurationManager\">\n\
           <configuration default=\"false\" name=\"{}\" type=\"ShConfigurationType\">\n\
             <option name=\"SCRIPT_TEXT\" value=\"{}\" />\n\
             <option name=\"INDEPENDENT_SCRIPT_PATH\" value=\"true\" />\n\
             <option name=\"SCRIPT_PATH\" value=\"\" />\n\
             <option name=\"SCRIPT_OPTIONS\" value=\"\" />\n\
             <option name=\"INDEPENDENT_SCRIPT_WORKING_DIRECTORY\" value=\"true\" />\n\
             <option name=\"SCRIPT_WORKING_DIRECTORY\" value=\"{}\" />\n\
             <option name=\"INDEPENDENT_INTERPRETER_PATH\" value=\"true\" />\n\
             <option name=\"INTERPRETER_PATH\" value=\"/bin/bash\" />\n\
             <option name=\"INTERPRETER_OPTIONS\" value=\"\" />\n\
             <option name=\"EXECUTE_IN_TERMINAL\" value=\"false\" />\n\
             <option name=\"EXECUTE_SCRIPT_FILE\" value=\"false\" />\n\
             <envs />\n\
             <method v=\"2\" />\n\
           </configuration>\n\
         </component>\n",
        xml_escape(name),
        xml_escape(&command),
        xml_escape(working_directory),
    )
}

#[cfg(not(windows))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
