//! Durable app-owned content state and operation receipts.
//!
//! This module owns serialization, atomic index replacement, and the cross-process
//! index lock. Content lifecycle operations decide when state changes; this module
//! owns how those changes are persisted.

use super::{
    CONTENT_STATE_SCHEMA, ContentLayout, ContentReference, Fingerprint, io, now_seconds, slug,
};
use crate::discovery::InstallationPaths;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct InstalledRecord {
    pub(super) artifact_id: String,
    pub(super) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) workshop_id: Option<String>,
    pub(super) enabled: bool,
    pub(super) installed_root: PathBuf,
    pub(super) fingerprint: Fingerprint,
    #[serde(default)]
    pub(super) dependencies: Vec<ContentReference>,
    #[serde(default)]
    pub(super) conflicts: Vec<ContentReference>,
    pub(super) installed_at: u64,
    pub(super) source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CacheRecord {
    pub(super) artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) workshop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) version: Option<String>,
    pub(super) cache_root: PathBuf,
    pub(super) fingerprint: Fingerprint,
    pub(super) acquired_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ContentIndex {
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) installed: BTreeMap<String, InstalledRecord>,
    #[serde(default)]
    pub(super) caches: BTreeMap<String, CacheRecord>,
}

impl Default for ContentIndex {
    fn default() -> Self {
        Self {
            schema_version: CONTENT_STATE_SCHEMA,
            installed: BTreeMap::new(),
            caches: BTreeMap::new(),
        }
    }
}

impl ContentIndex {
    pub(super) fn cache_by_selector(&self, selector: &str) -> Option<(String, &CacheRecord)> {
        let (identity, requested_version) = selector
            .rsplit_once('@')
            .map(|(identity, version)| (identity, Some(version)))
            .unwrap_or((selector, None));
        self.caches.iter().find_map(|(id, cache)| {
            ((id == identity
                || cache.artifact_id == identity
                || cache
                    .workshop_id
                    .as_deref()
                    .is_some_and(|workshop_id| workshop_id == identity))
                && requested_version
                    .is_none_or(|version| cache.version.as_deref() == Some(version)))
            .then(|| (id.clone(), cache))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OperationReceipt {
    pub(super) schema_version: u32,
    pub(super) action: String,
    pub(super) artifact_id: String,
    pub(super) status: String,
    pub(super) timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<String>,
}

pub(super) fn load_index(layout: &ContentLayout) -> Result<ContentIndex, String> {
    if !layout.index().is_file() {
        return Ok(ContentIndex::default());
    }
    let source =
        fs::read_to_string(layout.index()).map_err(io("read content index", layout.index()))?;
    let mut index: ContentIndex = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", layout.index().display()))?;
    if index.schema_version == 0 {
        index.schema_version = CONTENT_STATE_SCHEMA;
    }
    Ok(index)
}

pub(super) fn save_index(layout: &ContentLayout, index: &ContentIndex) -> Result<(), String> {
    if let Some(parent) = layout.index().parent() {
        fs::create_dir_all(parent).map_err(io("create content index parent", parent))?;
    }
    let encoded = toml::to_string_pretty(index)
        .map_err(|error| format!("failed to encode content index: {error}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = layout.index().with_extension(format!("tmp-{nonce}"));
    let backup = layout.index().with_extension(format!("backup-{nonce}"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io("create temporary content index", &temporary))?;
    file.write_all(encoded.as_bytes())
        .map_err(io("write temporary content index", &temporary))?;
    file.sync_all()
        .map_err(io("flush temporary content index", &temporary))?;
    drop(file);

    let had_previous = match fs::symlink_metadata(layout.index()) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::rename(layout.index(), &backup)
                .map_err(io("stage previous content index", layout.index()))?;
            true
        }
        Ok(_) => {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "content index is not a regular file: {}",
                layout.index().display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(io("inspect previous content index", layout.index())(error));
        }
    };
    if let Err(error) = fs::rename(&temporary, layout.index()) {
        if had_previous {
            let _ = fs::rename(&backup, layout.index());
        }
        let _ = fs::remove_file(&temporary);
        return Err(io("activate content index", layout.index())(error));
    }
    if had_previous {
        let _ = fs::remove_file(&backup);
    }
    Ok(())
}

pub(super) struct ContentIndexLock {
    pub(super) path: PathBuf,
}

impl Drop for ContentIndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn acquire_content_index_lock(
    layout: &ContentLayout,
) -> Result<ContentIndexLock, String> {
    if let Some(parent) = layout.index().parent() {
        fs::create_dir_all(parent).map_err(io("create content state directory", parent))?;
    }
    let path = layout.index().with_extension("lock");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            format!(
                "cannot acquire content index lock '{}': {error}",
                path.display()
            )
        })?;
    Ok(ContentIndexLock { path })
}

pub(super) fn write_lock(layout: &ContentLayout, record: &InstalledRecord) -> Result<(), String> {
    fs::create_dir_all(layout.locks()).map_err(io("create content locks", layout.locks()))?;
    let path = layout
        .locks()
        .join(format!("{}.toml", slug(&record.artifact_id)));
    let encoded = toml::to_string_pretty(record)
        .map_err(|error| format!("failed to encode content lock: {error}"))?;
    fs::write(&path, encoded).map_err(io("write content lock", &path))
}

pub(super) fn write_receipt(
    installation: &InstallationPaths,
    action: &str,
    artifact_id: &str,
    status: &str,
    detail: Option<&str>,
) -> Result<PathBuf, String> {
    let layout = ContentLayout::new(installation);
    fs::create_dir_all(layout.receipts())
        .map_err(io("create content receipts", layout.receipts()))?;
    let receipt = OperationReceipt {
        schema_version: CONTENT_STATE_SCHEMA,
        action: action.to_owned(),
        artifact_id: artifact_id.to_owned(),
        status: status.to_owned(),
        timestamp: now_seconds(),
        detail: detail.map(ToOwned::to_owned),
    };
    let path = layout.receipts().join(format!(
        "{}-{}-{}.toml",
        receipt.timestamp,
        action,
        slug(artifact_id)
    ));
    let encoded = toml::to_string_pretty(&receipt)
        .map_err(|error| format!("failed to encode content receipt: {error}"))?;
    fs::write(&path, encoded).map_err(io("write content receipt", &path))?;
    Ok(path)
}

pub(super) fn resolve_installed_selector(
    index: &ContentIndex,
    selector: &str,
) -> Result<String, String> {
    if let Some((id, version)) = selector.rsplit_once('@')
        && !id.is_empty()
        && !version.is_empty()
    {
        return index
            .installed
            .iter()
            .find_map(|(key, record)| {
                (record.artifact_id == id && record.version.as_deref() == Some(version))
                    .then(|| key.clone())
            })
            .ok_or_else(|| format!("content version is not installed: {selector}"));
    }
    index
        .installed
        .iter()
        .find_map(|(id, record)| {
            (id == selector
                || record.artifact_id == selector
                || record
                    .workshop_id
                    .as_deref()
                    .is_some_and(|workshop_id| workshop_id == selector))
            .then(|| id.clone())
        })
        .ok_or_else(|| format!("content is not installed: {selector}"))
}
