use super::{
    BUILD_FEATURES, GIT_TARGET, GIT_VERSION, GitNativeBuildError, MAX_NOTICE_FILE_BYTES,
    RECIPE_REVISION, SOURCE_BUNDLE_NAME, command_failed, io_error, remove_path, run_output,
    sha256_bytes, sha256_file, validate_archive_paths,
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
struct SourceBundleManifest {
    schema: u32,
    target: String,
    git_version: String,
    recipe_revision: String,
    compiler_identity: String,
    build_features: Vec<String>,
    redistribution_review: String,
    components: Vec<SourceBundleComponent>,
    notice_files: Vec<SourceBundleNotice>,
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
        fs::create_dir_all(&sources_directory)
            .map_err(|source| io_error("create Git source bundle archives", &sources_directory, source))?;
        fs::create_dir_all(&notices_directory)
            .map_err(|source| io_error("create Git source bundle notices", &notices_directory, source))?;

        let mut components = Vec::with_capacity(sources.len());
        let mut notice_files = Vec::new();
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
            let list_option = if source.extension == "xz" { "-tJf" } else { "-tzf" };
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
                let extract_option = if source.extension == "xz" { "-xJOf" } else { "-xzOf" };
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
                if notice.stdout.len() > MAX_NOTICE_FILE_BYTES || total_notice_bytes > MAX_NOTICE_FILE_BYTES * 8 {
                    return Err(GitNativeBuildError::Metadata(
                        "upstream license/notice material exceeds the bounded source-bundle limit".to_owned(),
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
                    .map_err(|error| io_error("write bundled upstream notice", &destination, error))?;
                let bundle_path = bundle_relative.to_string_lossy().replace('\\', "/");
                notice_files.push(SourceBundleNotice {
                    component: source.component.to_owned(),
                    upstream_path: archive_entry.to_owned(),
                    bundle_path: bundle_path.clone(),
                    sha256: sha256_bytes(&notice.stdout),
                });
                notice_document.push_str(&format!("\n===== {}: {} =====\n", source.component, archive_entry));
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
        if notice_files.is_empty() {
            return Err(GitNativeBuildError::Metadata(
                "no upstream license or notice files were discovered in the pinned source archives".to_owned(),
            ));
        }
        let manifest = SourceBundleManifest {
            schema: 1,
            target: GIT_TARGET.to_owned(),
            git_version: GIT_VERSION.to_owned(),
            recipe_revision: RECIPE_REVISION.to_owned(),
            compiler_identity: compiler_identity.to_owned(),
            build_features: BUILD_FEATURES
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
            redistribution_review: "incomplete: source texts and notice files are collected; binary-to-component mapping and legal review are still required".to_owned(),
            components,
            notice_files,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| {
                GitNativeBuildError::Metadata(format!(
                    "could not encode Git source inventory: {error}"
                ))
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
            "This archive contains the exact hash-pinned upstream source archives used by the matching Vapor-managed Linux Git build, plus extracted upstream license/notice texts and a machine-readable component inventory. The inventory is intentionally marked incomplete until every installed binary and linked component is mapped and redistribution obligations are reviewed. It is not a release approval.\n",
        )
        .map_err(|error| io_error("write Git source bundle README", &staging.join("README.txt"), error))?;

        let staging_arg = staging.to_string_lossy();
        let temporary_bundle_arg = temporary_bundle.to_string_lossy();
        let output = run_output(
            "package exact Git source and notice bundle",
            "tar",
            [
                "--sort=name",
                "--mtime=@0",
                "--owner=0",
                "--group=0",
                "--numeric-owner",
                "--format=ustar",
                "-cf",
                temporary_bundle_arg.as_ref(),
                "-C",
                staging_arg.as_ref(),
                ".",
            ],
            None,
            &[],
        )?;
        if !output.status.success() {
            return Err(command_failed("package exact Git source and notice bundle", output));
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .open(&temporary_bundle)
            .map_err(|error| io_error("open completed Git source bundle", &temporary_bundle, error))?;
        file.sync_all()
            .map_err(|error| io_error("sync completed Git source bundle", &temporary_bundle, error))?;
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
