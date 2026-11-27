//! Capture compiler link inputs for the exact Git ELF images staged by Vapor.

use super::{GitNativeBuildError, io_error, sha256_bytes};
use serde::Serialize;
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}};

#[derive(Debug, Clone, Serialize)]
pub(super) struct LinkEvidence {
    pub(super) binary_sha256: String,
    pub(super) invocation_sha256: String,
    pub(super) requested_static_libraries: Vec<String>,
    pub(super) archive_search_paths: Vec<String>,
    pub(super) link_flags: Vec<String>,
}

pub(super) fn collect_link_evidence(
    source_root: &Path,
    install_root: &Path,
) -> Result<Vec<LinkEvidence>, GitNativeBuildError> {
    let mut payload_hashes = BTreeSet::new();
    collect_elf_hashes(install_root, &mut payload_hashes)?;
    let mut evidence = BTreeMap::<String, LinkEvidence>::new();
    collect_invocations(source_root, &payload_hashes, &mut evidence)?;

    let missing = payload_hashes
        .iter()
        .filter(|digest| !evidence.contains_key(*digest))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(GitNativeBuildError::Metadata(format!(
            "compiler link-input evidence is missing for {} staged ELF image(s): {}",
            missing.len(),
            missing.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(evidence.into_values().collect())
}

fn collect_elf_hashes(
    root: &Path,
    hashes: &mut BTreeSet<String>,
) -> Result<(), GitNativeBuildError> {
    let entries = fs::read_dir(root)
        .map_err(|error| io_error("read Git payload for link attribution", root, error))?;
    for entry in entries {
        let path = entry
            .map_err(|error| io_error("read Git payload entry for link attribution", root, error))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect Git payload for link attribution", &path, error))?;
        if metadata.is_dir() {
            collect_elf_hashes(&path, hashes)?;
        } else if metadata.is_file() && is_elf(&path)? {
            hashes.insert(super::sha256_file(&path)
                .map_err(|error| io_error("hash Git ELF for link attribution", &path, error))?);
        }
    }
    Ok(())
}

fn collect_invocations(
    root: &Path,
    payload_hashes: &BTreeSet<String>,
    evidence: &mut BTreeMap<String, LinkEvidence>,
) -> Result<(), GitNativeBuildError> {
    let entries = fs::read_dir(root)
        .map_err(|error| io_error("read Git build tree for compiler link inputs", root, error))?;
    for entry in entries {
        let path = entry
            .map_err(|error| io_error("read Git build-tree entry for compiler link inputs", root, error))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect Git build-tree entry for compiler link inputs", &path, error))?;
        if metadata.is_dir() {
            collect_invocations(&path, payload_hashes, evidence)?;
            continue;
        }
        if !metadata.is_file() || !path.to_string_lossy().ends_with(".vapor.args") {
            continue;
        }
        let binary_path = PathBuf::from(path.to_string_lossy().trim_end_matches(".vapor.args"));
        if !binary_path.is_file() || !is_elf(&binary_path)? {
            continue;
        }
        let binary_sha256 = super::sha256_file(&binary_path)
            .map_err(|error| io_error("hash linked Git executable", &binary_path, error))?;
        if !payload_hashes.contains(&binary_sha256) {
            continue;
        }
        let arguments = fs::read(&path)
            .map_err(|error| io_error("read Git compiler link arguments", &path, error))?;
        let record = LinkEvidence {
            binary_sha256: binary_sha256.clone(),
            invocation_sha256: sha256_bytes(&arguments),
            requested_static_libraries: requested_libraries(&arguments),
            archive_search_paths: archive_search_paths(&arguments),
            link_flags: link_flags(&arguments),
        };
        if let Some(previous) = evidence.insert(binary_sha256.clone(), record.clone())
            && previous.invocation_sha256 != record.invocation_sha256
        {
            return Err(GitNativeBuildError::Metadata(format!(
                "identical Git ELF bytes have conflicting compiler link invocations: {binary_sha256}"
            )));
        }
    }
    Ok(())
}

fn arguments(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
}

fn requested_libraries(bytes: &[u8]) -> Vec<String> {
    arguments(bytes)
        .filter(|argument| argument.starts_with("-l") || *argument == "-pthread")
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn archive_search_paths(bytes: &[u8]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    let mut next_is_path = false;
    for argument in arguments(bytes) {
        if next_is_path {
            paths.insert(normalize_search_path(&argument));
            next_is_path = false;
        } else if argument == "-L" {
            next_is_path = true;
        } else if let Some(path) = argument.strip_prefix("-L") {
            paths.insert(normalize_search_path(path));
        }
    }
    paths.into_iter().collect()
}

fn normalize_search_path(path: &str) -> String {
    if path.contains("/dependencies/lib64") {
        "Vapor Git dependency prefix/lib64".to_owned()
    } else if path.contains("/dependencies/lib") {
        "Vapor Git dependency prefix/lib".to_owned()
    } else if path.contains("/tools/zig/") {
        "Vapor-managed Zig target sysroot".to_owned()
    } else {
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| format!("unclassified search path/{name}"))
            .unwrap_or_else(|| "unclassified search path".to_owned())
    }
}

fn link_flags(bytes: &[u8]) -> Vec<String> {
    arguments(bytes)
        .filter(|argument| matches!(argument.as_str(), "-static" | "-shared" | "-pie" | "-pthread"))
        .collect()
}

fn is_elf(path: &Path) -> Result<bool, GitNativeBuildError> {
    use std::io::Read as _;
    let mut magic = [0_u8; 4];
    let mut file = fs::File::open(path)
        .map_err(|error| io_error("inspect Git ELF signature", path, error))?;
    let count = file.read(&mut magic)
        .map_err(|error| io_error("read Git ELF signature", path, error))?;
    Ok(count == magic.len() && magic == *b"\x7fELF")
}
