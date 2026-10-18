//! Source workspace initialization and source-owned metadata repair.

use crate::{
    content::{self, ContentReference},
    discovery::{EnvironmentPaths, InstallationPaths},
    manifest::{self, ContentKind},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Options for creating a basic content workspace.
#[derive(Debug, Clone)]
pub(crate) struct BasicContentInit {
    pub(crate) path: PathBuf,
    pub(crate) organization: String,
    pub(crate) name: String,
    pub(crate) app_id: u32,
}

/// Summary of a newly created source workspace.
#[derive(Debug, Clone)]
pub(crate) struct InitReport {
    root: PathBuf,
    workspace_id: String,
    packagepack_id: String,
}

impl InitReport {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub(crate) fn packagepack_id(&self) -> &str {
        &self.packagepack_id
    }
}

/// One source metadata repair action.
#[derive(Debug, Clone)]
pub(crate) struct SourceRepairAction {
    manifest: PathBuf,
    section: String,
    reference_id: String,
    workshop_id: String,
}

impl SourceRepairAction {
    pub(crate) fn manifest(&self) -> &Path {
        &self.manifest
    }

    pub(crate) fn section(&self) -> &str {
        &self.section
    }

    pub(crate) fn reference_id(&self) -> &str {
        &self.reference_id
    }

    pub(crate) fn workshop_id(&self) -> &str {
        &self.workshop_id
    }
}

/// Source metadata repair summary.
#[derive(Debug, Clone)]
pub(crate) struct SourceRepairReport {
    actions: Vec<SourceRepairAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeneratedOutputKind {
    Binary,
    RuntimeLibrary,
    StaticCrate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GeneratedContentProject {
    Rust {
        crate_name: String,
        output: GeneratedOutputKind,
    },
    MetadataOnly,
}

impl SourceRepairReport {
    pub(crate) fn actions(&self) -> &[SourceRepairAction] {
        &self.actions
    }
}

/// Read the current app's Steam AppID for content templates.
pub(crate) fn installation_app_id(installation: &InstallationPaths) -> Result<u32, String> {
    let marker = installation.root().join(manifest::APP_FILE_NAME);
    let source = fs::read_to_string(&marker)
        .map_err(|error| format!("failed to read '{}': {error}", marker.display()))?;
    #[derive(Deserialize)]
    struct RootManifest {
        root: Option<RootSection>,
    }
    #[derive(Deserialize)]
    struct RootSection {
        steam: Option<RootSteam>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct RootSteam {
        app_id: u32,
    }
    let parsed: RootManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", marker.display()))?;
    parsed
        .root
        .and_then(|root| root.steam)
        .map(|steam| steam.app_id)
        .ok_or_else(|| {
            format!(
                "installation manifest has no [root.steam].app-id: {}",
                marker.display()
            )
        })
}

/// Create a basic engine/game/packagepack content workspace. Only the
/// code-bearing Engine and Game become Cargo packages; Packagepack is metadata.
pub(crate) fn init_basic_content(options: BasicContentInit) -> Result<InitReport, String> {
    validate_slug("organization", &options.organization)?;
    validate_slug("workspace name", &options.name)?;
    if options.app_id == 0 {
        return Err("Steam AppID must be non-zero".to_owned());
    }
    if options.path.exists()
        && fs::read_dir(&options.path)
            .map_err(io("read target workspace", &options.path))?
            .next()
            .is_some()
    {
        return Err(format!(
            "target workspace must be empty: {}",
            options.path.display()
        ));
    }
    let toolchain = vapor_core::ManagedToolchain::discover()
        .map_err(|error| {
            format!("cannot create a source workspace without a Vapor toolchain pin: {error}")
        })?
        .pin;
    fs::create_dir_all(&options.path).map_err(io("create target workspace", &options.path))?;

    let workspace_id = format!("{}/{}", options.organization, options.name);
    let engine = format!("{}-engine", options.name);
    let game = format!("{}-game", options.name);
    let packagepack = format!("{}-packagepack", options.name);
    let engine_id = format!("{workspace_id}/{engine}");
    let game_id = format!("{workspace_id}/{game}");
    let packagepack_id = format!("{workspace_id}/{packagepack}");
    let title = title_case(&options.name);
    let engine_crate = crate_name(&engine);

    write(
        &options.path.join(manifest::WORKSPACE_FILE_NAME),
        &format!(
            r#"schema = 1

[workspace]
name = "{name}"
organization = "{organization}"
version = "0.1.0"
repository = ""

[[workspace.projects]]
path = "crates/{engine}"

[[workspace.projects]]
path = "crates/{game}"

[[workspace.projects]]
path = "crates/{packagepack}"

[toolchain]
channel = "{toolchain_channel}"
version = "{toolchain_version}"
date = "{toolchain_date}"

[[project]]
name = "{engine}"
path = "crates/{engine}"

[[project]]
name = "{game}"
path = "crates/{game}"
"#,
            name = options.name,
            organization = options.organization,
            toolchain_channel = toolchain.channel,
            toolchain_version = toolchain.version,
            toolchain_date = toolchain.date,
        ),
    )?;
    write(
        &options.path.join("Cargo.toml"),
        r#"[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
"#,
    )?;
    write(
        &options.path.join("Cargo.lock"),
        &format!(
            r#"# This file is automatically @generated by Vapor.
version = 4

[[package]]
name = "{engine}"
version = "0.1.0"

[[package]]
name = "{game}"
version = "0.1.0"
dependencies = [
 "{engine}",
]
"#
        ),
    )?;
    write(&options.path.join(".gitignore"), "/target/\n/output/\n")?;
    write(
        &options.path.join("README.md"),
        &format!(
            r#"# {title}

Basic Vapor content workspace.

The Engine and Game contain Rust projects. The Packagepack is composition
metadata only and is not a Cargo member.

## Local loop

```text
vapor source open .
vapor content validate
vapor content deploy {packagepack_id} --select
```

## First Workshop publication

Create engine and game first, repair dependency Workshop IDs, then create the
packagepack.

```text
vapor content create {engine_id} --account ACCOUNT --yes
vapor source repair --write
vapor content create {game_id} --account ACCOUNT --yes
vapor source repair --write
vapor content create {packagepack_id} --account ACCOUNT --yes
```

For later updates:

```text
vapor source repair --write
vapor content publish {engine_id} {game_id} {packagepack_id} --account ACCOUNT --yes
```
"#
        ),
    )?;

    let engine_root = options.path.join("crates").join(&engine);
    write(
        &engine_root.join(manifest::ENGINE_FILE_NAME),
        &format!(
            r#"schema = 1

[engine]
name = "{engine}"
version.workspace = true
binaries = ["{engine}"]

[engine.steam]
app-id = {app_id}
visibility = "private"
title = "{title} Engine"
description = "Engine content for {title}."
tags = ["engine", "{name}"]
change-note = "Initial Vapor content upload."
"#,
            app_id = options.app_id,
            name = options.name,
        ),
    )?;
    write_package_manifest(&engine_root.join("Cargo.toml"), &engine, None, &[])?;
    write(
        &engine_root.join("src/lib.rs"),
        &format!(
            r#"#![forbid(unsafe_code)]

pub const CONTENT_ID: &str = "{engine_id}";
"#
        ),
    )?;
    write(
        &engine_root.join("src/main.rs"),
        &format!(
            r#"fn main() {{
    println!("{title} Engine");
}}
"#
        ),
    )?;

    let game_root = options.path.join("crates").join(&game);
    write(
        &game_root.join(manifest::GAME_FILE_NAME),
        &format!(
            r#"schema = 1

[game]
name = "{game}"
version.workspace = true
libraries = ["{game}"]

[game.engine]
id = "{engine_id}"

[game.steam]
app-id = {app_id}
visibility = "private"
title = "{title} Game"
description = "Game content for {title}."
tags = ["game", "{name}"]
change-note = "Initial Vapor content upload."
"#,
            app_id = options.app_id,
            name = options.name,
        ),
    )?;
    write_package_manifest(
        &game_root.join("Cargo.toml"),
        &game,
        Some("rlib\", \"cdylib"),
        &[(&engine, &format!("../{engine}"))],
    )?;
    write(
        &game_root.join("src/lib.rs"),
        &format!(
            r#"#![forbid(unsafe_code)]

pub const CONTENT_ID: &str = "{game_id}";

pub fn engine_content_id() -> &'static str {{
    {engine_crate}::CONTENT_ID
}}
"#
        ),
    )?;

    let packagepack_root = options.path.join("crates").join(&packagepack);
    write(
        &packagepack_root.join(manifest::PACKAGEPACK_FILE_NAME),
        &format!(
            r#"schema = 1

[packagepack]
name = "{packagepack}"
version.workspace = true

[packagepack.engine]
id = "{engine_id}"

[packagepack.game]
id = "{game_id}"

[packagepack.steam]
app-id = {app_id}
visibility = "private"
title = "{title} Packagepack"
description = "Playable packagepack for {title}."
tags = ["packagepack", "{name}"]
change-note = "Initial Vapor content upload."
"#,
            app_id = options.app_id,
            name = options.name,
        ),
    )?;

    Ok(InitReport {
        root: options.path,
        workspace_id,
        packagepack_id,
    })
}

/// Repair source-authored dependency Workshop IDs from sibling publish metadata.
pub(crate) fn repair_source_metadata(
    paths: &EnvironmentPaths,
    write_changes: bool,
) -> Result<SourceRepairReport, String> {
    let catalog = content::discover(paths)?;
    let workshop_ids = catalog
        .artifacts()
        .iter()
        .filter_map(|artifact| {
            artifact
                .workshop()
                .published_file_id()
                .map(|id| (artifact.id().to_owned(), id.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();

    let mut actions = Vec::new();
    for artifact in catalog.artifacts() {
        let mut source = fs::read_to_string(artifact.manifest())
            .map_err(io("read source manifest", artifact.manifest()))?;
        let mut changed = false;
        for dependency in artifact.dependencies() {
            let Some(workshop_id) = workshop_ids.get(dependency.id()) else {
                continue;
            };
            let Some(section) = dependency_section(artifact.kind(), dependency) else {
                continue;
            };
            let (updated, repaired) =
                repair_section_workshop_id(&source, &section, dependency.id(), workshop_id)?;
            if repaired {
                source = updated;
                changed = true;
                actions.push(SourceRepairAction {
                    manifest: artifact.manifest().to_path_buf(),
                    section,
                    reference_id: dependency.id().to_owned(),
                    workshop_id: workshop_id.clone(),
                });
            }
        }
        if changed && write_changes {
            write(artifact.manifest(), &source)?;
        }
    }

    Ok(SourceRepairReport { actions })
}

fn dependency_section(kind: ContentKind, reference: &ContentReference) -> Option<String> {
    match (kind, reference.relationship()) {
        (ContentKind::Game, "engine") => Some("game.engine".to_owned()),
        (ContentKind::Packagepack, "engine") => Some("packagepack.engine".to_owned()),
        (ContentKind::Packagepack, "game") => Some("packagepack.game".to_owned()),
        _ => None,
    }
}

fn repair_section_workshop_id(
    source: &str,
    section: &str,
    reference_id: &str,
    workshop_id: &str,
) -> Result<(String, bool), String> {
    let lines = source.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let mut section_start = None;
    let mut section_end = lines.len();
    let header = format!("[{section}]");

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if section_start.is_some() {
                section_end = index;
                break;
            }
            if trimmed == header {
                section_start = Some(index);
            }
        }
    }

    let Some(start) = section_start else {
        return Ok((source.to_owned(), false));
    };

    let mut id_line = None;
    let mut existing_workshop_line = None;
    for (index, line) in lines.iter().enumerate().take(section_end).skip(start + 1) {
        let trimmed = line.trim();
        if let Some((key, value)) = trimmed.split_once('=') {
            match key.trim() {
                "id" if unquote(value.trim()) == Some(reference_id) => id_line = Some(index),
                "workshop-id" => existing_workshop_line = Some(index),
                _ => {}
            }
        }
    }

    let Some(id_index) = id_line else {
        return Ok((source.to_owned(), false));
    };

    let mut updated = lines;
    let replacement = format!("workshop-id = \"{workshop_id}\"");
    if let Some(index) = existing_workshop_line {
        if updated[index].trim() == replacement {
            return Ok((source.to_owned(), false));
        }
        updated[index] = replacement;
    } else {
        updated.insert(id_index + 1, replacement);
    }
    let mut text = updated.join("\n");
    if source.ends_with('\n') {
        text.push('\n');
    }
    toml::from_str::<toml::Value>(&text)
        .map_err(|error| format!("source metadata repair produced invalid TOML: {error}"))?;
    Ok((text, true))
}

fn write_package_manifest(
    path: &Path,
    package_name: &str,
    crate_type: Option<&str>,
    dependencies: &[(&str, &str)],
) -> Result<(), String> {
    let mut source = format!(
        r#"[package]
name = "{package_name}"
version.workspace = true
edition.workspace = true
"#
    );
    if let Some(crate_type) = crate_type {
        source.push_str(&format!(
            r#"
[lib]
crate-type = ["{crate_type}"]
"#
        ));
    }
    if !dependencies.is_empty() {
        source.push_str("\n[dependencies]\n");
        for (name, path) in dependencies {
            source.push_str(&format!("{name} = {{ path = \"{path}\" }}\n"));
        }
    }
    write(path, &source)
}

/// Add the Rust source project for code-bearing content roles. Composition-only
/// roles intentionally contain no Cargo package or empty Rust placeholder.
pub(crate) fn scaffold_content_project(
    root: &Path,
    content_id: &str,
    version: &str,
    kind: ContentKind,
) -> Result<GeneratedContentProject, String> {
    let output = content_project_output(kind);
    let Some(output) = output else {
        return Ok(GeneratedContentProject::MetadataOnly);
    };
    let crate_name = format!("vapor_{}", content_id.replace(['/', '-'], "_"));
    let mut cargo = format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"{version}\"\nedition = \"2024\"\n"
    );
    if matches!(
        output,
        GeneratedOutputKind::RuntimeLibrary | GeneratedOutputKind::StaticCrate
    ) {
        let crate_types = match output {
            GeneratedOutputKind::RuntimeLibrary => "\"rlib\", \"cdylib\"",
            GeneratedOutputKind::StaticCrate => "\"rlib\", \"staticlib\"",
            GeneratedOutputKind::Binary => unreachable!("binary output has no library crate types"),
        };
        cargo.push_str(&format!("\n[lib]\ncrate-type = [{crate_types}]\n"));
    }
    write(&root.join("Cargo.toml"), &cargo)?;
    match output {
        GeneratedOutputKind::Binary => write(
            &root.join("src/main.rs"),
            &format!("fn main() {{ println!(\"Vapor Engine {content_id}\"); }}\n"),
        )?,
        GeneratedOutputKind::RuntimeLibrary | GeneratedOutputKind::StaticCrate => write(
            &root.join("src/lib.rs"),
            &format!("#![forbid(unsafe_code)]\n\npub const CONTENT_ID: &str = \"{content_id}\";\n"),
        )?,
    }
    Ok(GeneratedContentProject::Rust { crate_name, output })
}

pub(crate) fn content_project_output(kind: ContentKind) -> Option<GeneratedOutputKind> {
    match kind {
        ContentKind::Engine => Some(GeneratedOutputKind::Binary),
        ContentKind::Game
        | ContentKind::EngineMod
        | ContentKind::GameMod
        | ContentKind::ExtensionMod => Some(GeneratedOutputKind::RuntimeLibrary),
        ContentKind::Crate => Some(GeneratedOutputKind::StaticCrate),
        ContentKind::Packagepack
        | ContentKind::Enginepack
        | ContentKind::Gamepack
        | ContentKind::Modpack => None,
    }
}

fn validate_slug(label: &str, value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{label} must use lowercase ASCII letters, digits, and internal hyphens: {value}"
        ))
    }
}

fn title_case(value: &str) -> String {
    value
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn crate_name(value: &str) -> String {
    value.replace('-', "_")
}

fn unquote(value: &str) -> Option<&str> {
    value.strip_prefix('"')?.strip_suffix('"')
}

fn write(path: &Path, source: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io("create source parent", parent))?;
    }
    fs::write(path, source).map_err(io("write source file", path))
}

fn io<'a>(action: &'a str, path: &'a Path) -> impl Fn(std::io::Error) -> String + 'a {
    move |error| format!("failed to {action} '{}': {error}", path.display())
}
