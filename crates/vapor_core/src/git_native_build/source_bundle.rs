use super::{
    BUILD_FEATURES, GIT_TARGET, GIT_VERSION, GitNativeBuildError, MAX_NOTICE_FILE_BYTES,
    RECIPE_REVISION, SOURCE_BUNDLE_NAME, command_failed, io_error, remove_path, run_output,
    package_tar_gz, sha256_bytes, sha256_file, validate_archive_paths,
};
use super::link_evidence::LinkEvidence;
use serde::Serialize;
use std::{fs::{self, Metadata}, path::{Path, PathBuf}};
#[cfg(unix)]
use std::collections::BTreeMap;

const RECIPE_FILES: &[(&str, &[u8])] = &[
    (
        "src/git_native_build.rs",
        include_bytes!("../git_native_build.rs"),
    ),
    (
        "src/git_native_build/source_bundle.rs",
        include_bytes!("source_bundle.rs"),
    ),
    (
        "src/git_native_build/link_evidence.rs",
        include_bytes!("link_evidence.rs"),
    ),
    (
        "src/git_native_build/link_evidence_cc.sh",
        include_bytes!("link_evidence_cc.sh"),
    ),
];

#[derive(Debug, Clone, Serialize)]
struct SourceBundleManifest {
    schema: u32,
    target: String,
    git_version: String,
    recipe_revision: String,
    compiler_identity: String,
    packager_identity: String,
    build_features: Vec<String>,
    redistribution_review: String,
    components: Vec<SourceBundleComponent>,
    payload_files: Vec<PayloadFile>,
    link_invocations: Vec<LinkEvidence>,
    notice_files: Vec<SourceBundleNotice>,
    recipe_files: Vec<SourceBundleRecipeFile>,
}

#[derive(Debug, Clone, Serialize)]
struct PayloadFile {
    path: String,
    kind: &'static str,
    bytes: Option<u64>,
    sha256: Option<String>,
    mode: Option<String>,
    link_target: Option<String>,
    hardlink_to: Option<String>,
    components: Vec<&'static str>,
    link_invocation_sha256: Option<String>,
    requested_static_libraries: Option<Vec<String>>,
}

struct ScannedPayloadFile {
    record: PayloadFile,
    #[cfg(unix)]
    hardlink_key: Option<(u64, u64)>,
}
#[derive(Debug, Clone, Serialize)]
struct SourceBundleComponent {
    name: String,
    pub(super) version: String,
    pub(super) role: String,
    pub(super) source_url: String,
    source_archive: String,
    pub(super) source_sha256: String,
    license_text_identification: String,
}

#[derive(Debug, Clone, Serialize)]
struct SourceBundleNotice {
    pub(super) component: String,
    upstream_path: String,
    bundle_path: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct SourceBundleRecipeFile {
    path: String,
    bytes: u64,
    sha256: String,
}

pub(super) struct SourceArchiveInput<'a> {
    pub(super) component: &'a str,
    pub(super) version: &'a str,
    pub(super) role: &'a str,
    pub(super) source_url: &'a str,
    pub(super) source_sha256: &'a str,
    pub(super) archive: &'a Path,
    pub(super) top_level: &'a str,
    pub(super) extension: &'a str,
    pub(super) observed_license: &'a str,
}
pub(super) fn write_source_bundle(
    entry_root: &Path,
    sources: &[SourceArchiveInput<'_>],
    compiler_identity: &str,
    packager_identity: &str,
    install_root: &Path,
    link_evidence: &[LinkEvidence],
) -> Result<(String, u64), GitNativeBuildError> {
    let staging = entry_root.join("source-bundle.staging");
    let bundle = entry_root.join(SOURCE_BUNDLE_NAME);
    let temporary_bundle =
        entry_root.join(format!(".{SOURCE_BUNDLE_NAME}.{}.tmp", std::process::id()));
    remove_path(&staging)?;
    remove_path(&bundle)?;
    remove_path(&temporary_bundle)?;
    fs::create_dir_all(&staging)
        .map_err(|source| io_error("create Git source bundle staging", &staging, source))?;

    let result = (|| {
        let sources_directory = staging.join("sources");
        let notices_directory = staging.join("notices");
        let recipe_directory = staging.join("recipe");
        fs::create_dir_all(&sources_directory)
            .map_err(|source| {
                io_error("create Git source bundle archives", &sources_directory, source)
            })?;
        fs::create_dir_all(&notices_directory)
            .map_err(|source| {
                io_error("create Git source bundle notices", &notices_directory, source)
            })?;
        fs::create_dir_all(&recipe_directory)
            .map_err(|source| {
                io_error("create Git build recipe directory", &recipe_directory, source)
            })?;

        let mut components = Vec::with_capacity(sources.len());
        let mut notice_files = Vec::new();
        let mut recipe_files = Vec::with_capacity(RECIPE_FILES.len());
        let mut notice_document = String::from(
            "Vapor managed Git source bundle\n\nThe following upstream license and notice texts are reproduced from the exact source archives listed in components.json. This collected evidence is not a completed legal review and does not assert that every shipped binary component has been mapped.\n",
        );
        let mut total_notice_bytes = 0usize;

        for source in sources {
            let source_digest = sha256_file(source.archive)
                .map_err(|error| io_error("verify source bundle input", source.archive, error))?;
            if source_digest != source.source_sha256 {
                return Err(GitNativeBuildError::SourceDigestMismatch {
                    source: source.source_url.to_owned(),
                    expected: source.source_sha256.to_owned(),
                    actual: source_digest,
                });
            }
            let archive_name = source.archive.file_name().ok_or_else(|| {
                GitNativeBuildError::Metadata(format!(
                    "source archive has no filename: {}",
                    source.archive.display()
                ))
            })?;
            let bundled_archive = sources_directory.join(archive_name);
            fs::copy(source.archive, &bundled_archive)
                .map_err(|error| {
                    io_error(
                        "copy verified source archive into bundle",
                        &bundled_archive,
                        error,
                    )
                })?;

            validate_archive_paths(source.archive, source.top_level, source.extension == "xz")?;
            let list_option = if source.extension == "xz" {
                "-tJf"
            } else {
                "-tzf"
            };
            let archive_arg = source.archive.to_string_lossy();
            let listing = run_output(
                "list upstream license and notice files",
                "tar",
                [list_option, archive_arg.as_ref()],
                None,
                &[],
            )?;
            if !listing.status.success() {
                return Err(command_failed("list upstream license and notice files", listing));
            }

            let component_slug = source.component.to_ascii_lowercase().replace(' ', "-");
            for archive_entry in String::from_utf8_lossy(&listing.stdout).lines() {
                let entry_path = Path::new(archive_entry);
                if archive_entry.ends_with('/') || !is_license_or_notice_path(entry_path) {
                    continue;
                }
                let mut components_iter = entry_path.components();
                let _top = components_iter.next();
                let relative_path = components_iter.collect::<PathBuf>();
                if relative_path.as_os_str().is_empty() {
                    continue;
                }
                let extract_option = if source.extension == "xz" {
                    "-xJOf"
                } else {
                    "-xzOf"
                };
                let notice = run_output(
                    "read upstream license and notice text",
                    "tar",
                    [extract_option, archive_arg.as_ref(), archive_entry],
                    None,
                    &[],
                )?;
                if !notice.status.success() {
                    return Err(command_failed("read upstream license and notice text", notice));
                }
                total_notice_bytes = total_notice_bytes.saturating_add(notice.stdout.len());
                if notice.stdout.len() > MAX_NOTICE_FILE_BYTES
                    || total_notice_bytes > MAX_NOTICE_FILE_BYTES * 8
                {
                    return Err(GitNativeBuildError::Metadata(
                        "upstream license/notice material exceeds the bounded source-bundle limit"
                            .to_owned(),
                    ));
                }
                let bundle_relative = PathBuf::from("notices")
                    .join(&component_slug)
                    .join(&relative_path);
                let destination = staging.join(&bundle_relative);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| {
                            io_error("create bundled notice directory", parent, error)
                        })?;
                }
                fs::write(&destination, &notice.stdout)
                    .map_err(|error| {
                        io_error("write bundled upstream notice", &destination, error)
                    })?;
                let bundle_path = bundle_relative.to_string_lossy().replace('\\', "/");
                notice_files.push(SourceBundleNotice {
                    component: source.component.to_owned(),
                    upstream_path: archive_entry.to_owned(),
                    bundle_path: bundle_path.clone(),
                    sha256: sha256_bytes(&notice.stdout),
                });
                notice_document.push_str(&format!(
                    "\n===== {}: {} =====\n",
                    source.component, archive_entry
                ));
                notice_document.push_str(&String::from_utf8_lossy(&notice.stdout));
                if !notice_document.ends_with('\n') {
                    notice_document.push('\n');
                }
            }

            components.push(SourceBundleComponent {
                name: source.component.to_owned(),
                version: source.version.to_owned(),
                role: source.role.to_owned(),
                source_url: source.source_url.to_owned(),
                source_archive: format!("sources/{}", archive_name.to_string_lossy()),
                source_sha256: source.source_sha256.to_owned(),
                license_text_identification: source.observed_license.to_owned(),
            });
        }
        for (relative_path, contents) in RECIPE_FILES {
            let relative_path = Path::new(relative_path);
            let destination = recipe_directory.join(relative_path);
            let parent = destination.parent().ok_or_else(|| {
                GitNativeBuildError::Metadata(format!(
                    "build recipe path has no parent: {}",
                    destination.display()
                ))
            })?;
            fs::create_dir_all(parent)
                .map_err(|error| io_error("create build recipe directory", parent, error))?;
            fs::write(&destination, contents)
                .map_err(|error| {
                    io_error("write bundled build recipe source", &destination, error)
                })?;
            recipe_files.push(SourceBundleRecipeFile {
                path: format!(
                    "recipe/{}",
                    relative_path.to_string_lossy().replace('\\', "/")
                ),
                bytes: contents.len() as u64,
                sha256: sha256_bytes(contents),
            });
        }
        if notice_files.is_empty() {
            return Err(GitNativeBuildError::Metadata(
                "no upstream license or notice files were discovered in the pinned source archives"
                    .to_owned(),
            ));
        }
        let payload_files = payload_inventory(install_root, link_evidence)?;
        let manifest = SourceBundleManifest {
            schema: 4,
            target: GIT_TARGET.to_owned(),
            git_version: GIT_VERSION.to_owned(),
            recipe_revision: RECIPE_REVISION.to_owned(),
            compiler_identity: compiler_identity.to_owned(),
            packager_identity: packager_identity.to_owned(),
            build_features: BUILD_FEATURES
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
            redistribution_review: "incomplete: installed paths are attributed to Git; per-ELF compiler-requested libraries are recorded but actual static archive-member inclusion is unknown; license scope, notices, corresponding-source sufficiency, and redistribution terms still require review".to_owned(),
            components,
            payload_files,
            link_invocations: link_evidence.to_vec(),
            notice_files,
            recipe_files,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| {
                GitNativeBuildError::Metadata(format!("could not encode Git source inventory: {error}"))
            })?;
        fs::write(staging.join("components.json"), manifest_bytes)
            .map_err(|error| {
                io_error(
                    "write Git source inventory",
                    &staging.join("components.json"),
                    error,
                )
            })?;
        fs::write(staging.join("THIRD_PARTY_NOTICES.txt"), notice_document)
            .map_err(|error| {
                io_error(
                    "write combined Git notices",
                    &staging.join("THIRD_PARTY_NOTICES.txt"),
                    error,
                )
            })?;
        fs::write(
            staging.join("README.txt"),
            "This archive contains the exact hash-pinned upstream source archives used by the matching Vapor-managed Linux Git build, extracted upstream license/notice texts, the exact Vapor Core build-recipe source files, and a machine-readable component inventory. Recipe-file, notice, and compiler-invocation digests are recorded in components.json. The inventory maps installed paths to the Git source and records compiler-requested static library selectors for each ELF image. It does not establish which archive members were linked into each image. License scope, notices, corresponding-source sufficiency, and redistribution terms remain subject to review. It is not a release approval.\n",
        )
        .map_err(|error| io_error("write Git source bundle README", &staging.join("README.txt"), error))?;

        let _ = package_tar_gz(
            &staging,
            &temporary_bundle,
            "package exact Git source and notice bundle",
        )?;
        fs::rename(&temporary_bundle, &bundle)
            .map_err(|error| io_error("promote completed Git source bundle", &bundle, error))?;
        let bytes = fs::metadata(&bundle)
            .map_err(|error| io_error("inspect completed Git source bundle", &bundle, error))?
            .len();
        let digest = sha256_file(&bundle)
            .map_err(|error| io_error("hash completed Git source bundle", &bundle, error))?;
        Ok((digest, bytes))
    })();

    if result.is_err() {
        let _ = remove_path(&temporary_bundle);
        let _ = remove_path(&bundle);
    }
    let _ = remove_path(&staging);
    result
}

fn payload_inventory(
    root: &Path,
    link_evidence: &[LinkEvidence],
) -> Result<Vec<PayloadFile>, GitNativeBuildError> {
    let root = fs::canonicalize(root)
        .map_err(|error| io_error("resolve Git payload inventory", root, error))?;
    let mut scanned = Vec::new();
    scan_payload_directory(&root, &root, &mut scanned)?;
    scanned.sort_by(|left, right| left.record.path.cmp(&right.record.path));

    #[cfg(unix)]
    let mut hardlink_owners: BTreeMap<(u64, u64), String> = BTreeMap::new();
    let mut records = Vec::with_capacity(scanned.len());
    for mut file in scanned {
        #[cfg(unix)]
        if let Some(key) = file.hardlink_key {
            if let Some(owner) = hardlink_owners.get(&key) {
                file.record.hardlink_to = Some(owner.clone());
            } else {
                hardlink_owners.insert(key, file.record.path.clone());
            }
        }
        records.push(file.record);
    }
    if records.is_empty() {
        return Err(GitNativeBuildError::Metadata(
            "Git payload inventory contains no installed entries".to_owned(),
        ));
    }
    let evidence_by_binary = link_evidence
        .iter()
        .map(|evidence| (evidence.binary_sha256.as_str(), evidence))
        .collect::<std::collections::BTreeMap<_, _>>();
    for record in &mut records {
        if record.kind != "elf" {
            continue;
        }
        let digest = record.sha256.as_deref().ok_or_else(|| {
            GitNativeBuildError::Metadata(format!(
                "ELF payload lacks a content digest: {}",
                record.path
            ))
        })?;
        let evidence = evidence_by_binary.get(digest).ok_or_else(|| {
            GitNativeBuildError::Metadata(format!(
                "ELF payload lacks compiler link-invocation evidence: {}",
                record.path
            ))
        })?;
        record.link_invocation_sha256 = Some(evidence.invocation_sha256.clone());
        record.requested_static_libraries = Some(evidence.requested_static_libraries.clone());
    }
    Ok(records)
}

fn scan_payload_directory(
    root: &Path,
    directory: &Path,
    records: &mut Vec<ScannedPayloadFile>,
) -> Result<(), GitNativeBuildError> {
    let mut children = fs::read_dir(directory)
        .map_err(|error| io_error("read Git payload inventory", directory, error))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error("read Git payload entry", directory, error))?;
    children.sort();
    for path in children {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| GitNativeBuildError::InventoryEscapesRoot(path.clone()))?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect Git payload entry", &path, error))?;
        if metadata.is_dir() {
            scan_payload_directory(root, &path, records)?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)
                .map_err(|error| io_error("read Git payload symlink", &path, error))?;
            let target = target.to_string_lossy().replace('\\', "/");
            if target.starts_with('/') || target.split('/').any(|part| part == "..") {
                return Err(GitNativeBuildError::InvalidInstallSymlink(path));
            }
            records.push(ScannedPayloadFile {
                record: PayloadFile {
                    path: relative,
                    kind: "symlink",
                    bytes: None,
                    sha256: None,
                    mode: None,
                    link_target: Some(target),
                    hardlink_to: None,
                    components: vec!["Git"],
                    link_invocation_sha256: None,
                    requested_static_libraries: None,
                },
                #[cfg(unix)]
                hardlink_key: None,
            });
        } else if metadata.is_file() {
            records.push(payload_file_record(path, relative, &metadata)?);
        } else {
            return Err(GitNativeBuildError::UnsupportedInstallEntry(path));
        }
    }
    Ok(())
}

fn payload_file_record(
    path: PathBuf,
    relative: String,
    metadata: &Metadata,
) -> Result<ScannedPayloadFile, GitNativeBuildError> {
    let digest = sha256_file(&path)
        .map_err(|error| io_error("hash Git payload file", &path, error))?;
    let mut magic = [0u8; 4];
    use std::io::Read as _;
    let read = fs::File::open(&path)
        .and_then(|mut file| file.read(&mut magic))
        .map_err(|error| io_error("inspect Git payload file", &path, error))?;
    let is_elf = read == magic.len() && magic == *b"\x7fELF";
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt as _;
        Some(format!("{:04o}", metadata.permissions().mode() & 0o7777))
    };
    #[cfg(not(unix))]
    let mode = None;

    #[cfg(unix)]
    let hardlink_key = {
        use std::os::unix::fs::MetadataExt as _;
        (metadata.nlink() > 1).then(|| (metadata.dev(), metadata.ino()))
    };

    Ok(ScannedPayloadFile {
        record: PayloadFile {
            path: relative,
            kind: if is_elf { "elf" } else { "file" },
            bytes: Some(metadata.len()),
            sha256: Some(digest),
            mode,
            link_target: None,
            hardlink_to: None,
            components: vec!["Git"],
            link_invocation_sha256: None,
            requested_static_libraries: None,
        },
        #[cfg(unix)]
        hardlink_key,
    })
}

fn is_license_or_notice_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    ["copying", "license", "licence", "notice"]
        .iter()
        .any(|stem| {
            name == *stem
                || name.starts_with(&format!("{stem}."))
                || name.starts_with(&format!("{stem}_"))
                || name.starts_with(&format!("{stem}-"))
        })
}
