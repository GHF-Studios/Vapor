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
        let run_root = project_root.join(".run/Vapor");
        let vapor = vapor_executable(installation_root);
        let mut configurations = Vec::new();

        for (directory, folder, filename, name, arguments, terminal) in [
            (
                "Installation",
                "Vapor · Installation",
                "Diagnose.run.xml",
                "Diagnose",
                "installation diagnose",
                false,
            ),
            (
                "Installation",
                "Vapor · Installation",
                "Repair.run.xml",
                "Repair",
                "installation repair",
                false,
            ),
            (
                "Client",
                "Vapor · Client",
                "Status.run.xml",
                "Status",
                "client status",
                false,
            ),
            (
                "Client",
                "Vapor · Client",
                "Build.run.xml",
                "Build",
                "client build",
                false,
            ),
            (
                "Client",
                "Vapor · Client",
                "Test.run.xml",
                "Test",
                "client test",
                false,
            ),
            (
                "Client",
                "Vapor · Client",
                "Deploy_Local.run.xml",
                "Deploy Local",
                "client deploy local",
                false,
            ),
            (
                "Client",
                "Vapor · Client",
                "Deploy_Steam.run.xml",
                "Deploy Steam",
                "client deploy steam",
                true,
            ),
            (
                "Platform-Server",
                "Vapor · Platform Server",
                "Status.run.xml",
                "Status",
                "platform-server status",
                false,
            ),
            (
                "Platform-Server",
                "Vapor · Platform Server",
                "Runs.run.xml",
                "Runs",
                "platform-server runs",
                false,
            ),
            (
                "Platform-Server",
                "Vapor · Platform Server",
                "Build.run.xml",
                "Build",
                "platform-server build",
                false,
            ),
            (
                "Platform-Server",
                "Vapor · Platform Server",
                "Test.run.xml",
                "Test",
                "platform-server test",
                false,
            ),
            (
                "Platform-Server",
                "Vapor · Platform Server",
                "Deploy.run.xml",
                "Deploy",
                "platform-server deploy",
                true,
            ),
        ] {
            configurations.push((
                run_root.join(directory).join(filename),
                shell_configuration_at(
                    name,
                    folder,
                    &vapor,
                    arguments,
                    "$PROJECT_DIR$",
                    terminal,
                ),
            ));
        }

        if let Some(loo_cast_root) = child_named(project_root, "Loo-Cast") {
            let working_directory = project_path(project_root, &loo_cast_root);
            let loo_cast_run_root = run_root.join("Loo-Cast");

            for (filename, name, arguments, terminal) in [
                ("Inspect.run.xml", "Inspect", "packagepack inspect", false),
                ("Resolve.run.xml", "Resolve", "packagepack resolve", false),
                ("Build.run.xml", "Build", "packagepack build", false),
                ("Run.run.xml", "Run", "packagepack run", true),
            ] {
                configurations.push((
                    loo_cast_run_root.join(filename),
                    shell_configuration_at(
                        name,
                        "Vapor · Loo Cast",
                        &vapor,
                        arguments,
                        &working_directory,
                        terminal,
                    ),
                ));
            }
        }

        configurations.extend(client_run_configurations(project_root, &run_root, &vapor));

        configurations.push((
            run_root.join("Content/Other_Content.run.xml"),
            shell_script_configuration_at(
                "Other Content…",
                "Vapor · Content",
                &interactive_content_script(&vapor),
                "$PROJECT_DIR$",
                true,
            ),
        ));

        configurations
    }
}

pub(super) fn obsolete(project_root: &Path) -> Vec<PathBuf> {
    let run_root = project_root.join(".run");
    let Ok(entries) = fs::read_dir(&run_root) else {
        return Vec::new();
    };

    let mut obsolete = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_str()?;

            (path.is_file() && name.starts_with("Vapor_") && name.ends_with(".run.xml"))
                .then_some(path)
        })
        .collect::<Vec<_>>();

    obsolete.sort();
    obsolete
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
                run_root
                    .join("Client/Run")
                    .join(format!("{}.run.xml", file_id(&suffix))),
                shell_configuration_at(
                    &format!("Run · {label}"),
                    "Vapor · Client",
                    vapor,
                    &arguments,
                    &project_path(project_root, &package_root),
                    true,
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
fn vapor_executable(installation_root: &Path) -> PathBuf {
    let bin = installation_root.join("bin");

    if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        return bin.join("x86_64-unknown-linux-gnu/vapor");
    }

    if cfg!(all(
        target_arch = "x86_64",
        target_os = "windows",
        target_env = "msvc"
    )) {
        return bin.join("x86_64-pc-windows-msvc/vapor.exe");
    }

    bin.join("vapor")
}

#[cfg(not(windows))]
fn interactive_content_script(vapor: &Path) -> String {
    let mut script = format!(
        "set -euo pipefail\nvapor={}\n\n",
        shell_quote(&vapor.to_string_lossy()),
    );

    script.push_str(
        r#"printf 'Content type? Possible options:\n'
printf '  packagepack\n  enginepack\n  gamepack\n  modpack\n'
printf '  engine\n  game\n  engine-mod\n  game-mod\n  extension-mod\n  library\n> '
IFS= read -r kind

case "$kind" in
  packagepack) operations='inspect resolve build run' ;;
  enginepack|gamepack|modpack) operations='inspect resolve' ;;
  engine|game|engine-mod|game-mod|extension-mod) operations='inspect' ;;
  library) operations='inspect resolve verify repair' ;;
  *) printf 'Unknown content type: %s\n' "$kind" >&2; exit 2 ;;
esac

printf '\nAvailable %s content:\n' "$kind"
"$vapor" "$kind" list || true

printf '\nOperation? Possible options: %s\n> ' "$operations"
IFS= read -r operation
case " $operations " in
  *" $operation "*) ;;
  *) printf 'Unsupported %s operation: %s\n' "$kind" "$operation" >&2; exit 2 ;;
esac

printf '\nContent ID? Leave blank only if Vapor can infer it from context.\n> '
IFS= read -r id

command=("$vapor" "$kind" "$operation")
if [[ -n "$id" ]]; then
  command+=("$id")
fi

printf '\n→'
printf ' %q' "${command[@]}"
printf '\n\n'
exec "${command[@]}"
"#,
    );

    script
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
fn shell_configuration_at(
    name: &str,
    folder: &str,
    vapor: &Path,
    arguments: &str,
    working_directory: &str,
    terminal: bool,
) -> String {
    let command = format!("{} {arguments}", shell_quote(&vapor.to_string_lossy()));

    shell_script_configuration_at(name, folder, &command, working_directory, terminal)
}

#[cfg(not(windows))]
fn shell_script_configuration_at(
    name: &str,
    folder: &str,
    script: &str,
    working_directory: &str,
    terminal: bool,
) -> String {
    format!(
        "<component name=\"ProjectRunConfigurationManager\">\n\
           <configuration default=\"false\" name=\"{}\" type=\"ShConfigurationType\" folderName=\"{}\">\n\
             <option name=\"SCRIPT_TEXT\" value=\"{}\" />\n\
             <option name=\"INDEPENDENT_SCRIPT_PATH\" value=\"true\" />\n\
             <option name=\"SCRIPT_PATH\" value=\"\" />\n\
             <option name=\"SCRIPT_OPTIONS\" value=\"\" />\n\
             <option name=\"INDEPENDENT_SCRIPT_WORKING_DIRECTORY\" value=\"true\" />\n\
             <option name=\"SCRIPT_WORKING_DIRECTORY\" value=\"{}\" />\n\
             <option name=\"INDEPENDENT_INTERPRETER_PATH\" value=\"true\" />\n\
             <option name=\"INTERPRETER_PATH\" value=\"/bin/bash\" />\n\
             <option name=\"INTERPRETER_OPTIONS\" value=\"\" />\n\
             <option name=\"EXECUTE_IN_TERMINAL\" value=\"{}\" />\n\
             <option name=\"EXECUTE_SCRIPT_FILE\" value=\"false\" />\n\
             <envs />\n\
             <method v=\"2\" />\n\
           </configuration>\n\
         </component>\n",
        xml_escape(name),
        xml_escape(folder),
        xml_script(script),
        xml_escape(working_directory),
        terminal,
    )
}

#[cfg(not(windows))]
fn xml_script(value: &str) -> String {
    xml_escape(value)
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
}

#[cfg(not(windows))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
