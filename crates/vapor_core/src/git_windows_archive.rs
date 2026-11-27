//! Bounded, non-authoritative inventory of the pinned official Windows MinGit archive.
//!
//! Acquisition and inspection are deliberately cache-only: this path does not
//! extract files, activate a provider, or publish a redistribution payload.

use ring::digest::{Context, SHA256};
use serde::Serialize;
use flate2::read::DeflateDecoder;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

use crate::VaporInstallation;

const MINGIT_VERSION: &str = "2.56.1.windows.1";
const MINGIT_ASSET: &str = "MinGit-2.56.1-64-bit.zip";
const MINGIT_URL: &str = "https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/MinGit-2.56.1-64-bit.zip";
const MINGIT_SHA256: &str = "4a92850b6e917da4fd010669b614b55dccf5b0ec333a534cc4be4700dfefdafd";
const CACHE_RELATIVE_ROOT: &str = "cache/build/native-tools/git/windows";
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct GitWindowsArchiveReport {
    pub version: String,
    pub asset: String,
    pub archive_path: PathBuf,
    pub archive_bytes: u64,
    pub archive_sha256: String,
    pub inventory_path: PathBuf,
    pub file_count: usize,
    pub expanded_bytes: u64,
    pub executables: Vec<String>,
    pub dlls: Vec<String>,
    pub license_and_notice_paths: Vec<String>,
    pub package_versions: Option<String>,
    pub libexec_moved: Option<String>,
    pub expected_paths: Vec<ExpectedPath>,
    pub files: Vec<ArchiveFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpectedPath {
    pub path: &'static str,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchiveFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// Download, hash-verify, and inventory the pinned MinGit ZIP in Vapor User Data.
pub fn inspect_windows_mingit(
    installation: &VaporInstallation,
) -> Result<GitWindowsArchiveReport, GitWindowsArchiveError> {
    let root = installation.user_data_root().join(CACHE_RELATIVE_ROOT).join(MINGIT_VERSION);
    fs::create_dir_all(&root).map_err(|source| io_error("create MinGit cache directory", &root, source))?;
    let archive_path = root.join(MINGIT_ASSET);
    let archive_bytes = acquire_archive(&archive_path)?;
    let archive_sha256 = sha256_file(&archive_path)?;
    let mut file = File::open(&archive_path).map_err(|source| io_error("open MinGit archive", &archive_path, source))?;
    let entries = read_central_directory(&mut file)?;
    if entries.len() > MAX_ENTRIES {
        return Err(GitWindowsArchiveError::Limit(format!("archive has {} entries; limit is {MAX_ENTRIES}", entries.len())));
    }
    let mut files = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    let mut expanded_bytes = 0_u64;
    let mut executables = Vec::new();
    let mut dlls = Vec::new();
    let mut license_and_notice_paths = Vec::new();
    let mut package_versions = None;
    let mut libexec_moved = None;

    for entry in entries {
        let name = entry.name;
        validate_archive_path(&name)?;
        if !seen.insert(name.clone()) {
            return Err(GitWindowsArchiveError::UnsafePath(format!("duplicate path {name:?}")));
        }
        if entry.is_directory {
            continue;
        }

        let declared_size = entry.uncompressed_size;
        expanded_bytes = expanded_bytes.checked_add(declared_size).ok_or_else(|| {
            GitWindowsArchiveError::Limit("expanded byte count overflowed".to_owned())
        })?;
        if expanded_bytes > MAX_EXPANDED_BYTES {
            return Err(GitWindowsArchiveError::Limit(format!(
                "expanded data exceeds {MAX_EXPANDED_BYTES} bytes"
            )));
        }
        let mut context = Context::new(&SHA256);
        let mut crc = crc32fast::Hasher::new();
        let mut captured = Vec::new();
        let capture_metadata = matches!(name.as_str(), "etc/package-versions.txt" | "etc/libexec-moved.txt");
        let mut local_header = [0_u8; 30];
        file.seek(SeekFrom::Start(entry.local_header_offset)).map_err(|source| io_error("seek MinGit local header", &archive_path, source))?;
        file.read_exact(&mut local_header).map_err(|source| io_error("read MinGit local header", &archive_path, source))?;
        if le_u32(&local_header, 0) != 0x0403_4b50 || le_u16(&local_header, 8) != entry.compression_method {
            return Err(GitWindowsArchiveError::Archive(format!("invalid local header for {name}")));
        }
        let local_name_len = le_u16(&local_header, 26) as u64;
        let local_extra_len = le_u16(&local_header, 28) as u64;
        let mut local_name = vec![0_u8; local_name_len as usize];
        file.read_exact(&mut local_name).map_err(|source| io_error("read MinGit local filename", &archive_path, source))?;
        if local_name != name.as_bytes() {
            return Err(GitWindowsArchiveError::Archive(format!("central/local path mismatch for {name}")));
        }
        let data_offset = entry.local_header_offset.checked_add(30 + local_name_len + local_extra_len)
            .ok_or_else(|| GitWindowsArchiveError::Limit(format!("offset overflow for {name}")))?;
        file.seek(SeekFrom::Start(data_offset)).map_err(|source| io_error("seek MinGit file data", &archive_path, source))?;
        let bounded_data = (&mut file).take(entry.compressed_size);
        let mut source: Box<dyn Read + '_> = match entry.compression_method {
            0 => Box::new(bounded_data),
            8 => Box::new(DeflateDecoder::new(bounded_data)),
            method => return Err(GitWindowsArchiveError::Archive(format!("unsupported compression method {method} for {name}"))),
        };
        let mut buffer = [0_u8; 64 * 1024];
        let mut actual_size = 0_u64;
        loop {
            let read_size = source.read(&mut buffer)
                .map_err(|error| GitWindowsArchiveError::Archive(format!("read {name}: {error}")))?;
            if read_size == 0 { break; }
            actual_size = actual_size.checked_add(read_size as u64).ok_or_else(|| {
                GitWindowsArchiveError::Limit(format!("size overflow while reading {name}"))
            })?;
            if actual_size > declared_size {
                return Err(GitWindowsArchiveError::Archive(format!("{name} expanded beyond its declared size")));
            }
            context.update(&buffer[..read_size]);
            crc.update(&buffer[..read_size]);
            if capture_metadata && captured.len() as u64 + read_size as u64 <= MAX_METADATA_BYTES {
                captured.extend_from_slice(&buffer[..read_size]);
            } else if capture_metadata {
                return Err(GitWindowsArchiveError::Limit(format!("metadata file {name} exceeds {MAX_METADATA_BYTES} bytes")));
            }
        }
        if actual_size != declared_size {
            return Err(GitWindowsArchiveError::Archive(format!(
                "{name} expanded to {actual_size} bytes, expected {declared_size}"
            )));
        }
        if crc.finalize() != entry.crc32 {
            return Err(GitWindowsArchiveError::Archive(format!("CRC-32 mismatch for {name}")));
        }
        let digest = hex(context.finish().as_ref());
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".exe") { executables.push(name.clone()); }
        if lower.ends_with(".dll") { dlls.push(name.clone()); }
        if is_license_or_notice(&name) { license_and_notice_paths.push(name.clone()); }
        if name == "etc/package-versions.txt" {
            package_versions = Some(String::from_utf8_lossy(&captured).into_owned());
        }
        if name == "etc/libexec-moved.txt" {
            libexec_moved = Some(String::from_utf8_lossy(&captured).into_owned());
        }
        files.push(ArchiveFile { path: name, bytes: declared_size, sha256: digest });
    }

    let paths = files.iter().map(|file| file.path.as_str()).collect::<BTreeSet<_>>();
    let expected_paths = [
        "cmd/git.exe",
        "ucrt64/bin/git.exe",
        "ucrt64/bin/git-remote-https.exe",
        "ucrt64/bin/git-http-fetch.exe",
        "ucrt64/libexec/git-core/git-credential-wincred.exe",
        "etc/libexec-moved.txt",
        "etc/package-versions.txt",
        "mingw64/bin/git.exe",
        "mingw64/libexec/git-core/git.exe",
    ].into_iter().map(|path| ExpectedPath { path, present: paths.contains(path) }).collect();

    let report = GitWindowsArchiveReport {
        version: MINGIT_VERSION.to_owned(),
        asset: MINGIT_ASSET.to_owned(),
        archive_path: archive_path.clone(),
        archive_bytes,
        archive_sha256,
        inventory_path: root.join("inventory.json"),
        file_count: files.len(),
        expanded_bytes,
        executables,
        dlls,
        license_and_notice_paths,
        package_versions,
        libexec_moved,
        expected_paths,
        files,
    };
    let inventory = serde_json::to_vec_pretty(&report)
        .map_err(|error| GitWindowsArchiveError::Metadata(error.to_string()))?;
    let inventory_tmp = root.join(format!(".inventory-{}.tmp", std::process::id()));
    fs::write(&inventory_tmp, inventory)
        .map_err(|source| io_error("write MinGit inventory", &inventory_tmp, source))?;
    fs::rename(&inventory_tmp, &report.inventory_path)
        .map_err(|source| io_error("promote MinGit inventory", &report.inventory_path, source))?;
    Ok(report)
}

struct CentralEntry {
    name: String,
    is_directory: bool,
    compression_method: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_header_offset: u64,
}

fn read_central_directory(file: &mut File) -> Result<Vec<CentralEntry>, GitWindowsArchiveError> {
    let file_len = file.metadata().map_err(|source| io_error("read MinGit archive metadata", Path::new(MINGIT_ASSET), source))?.len();
    if file_len > MAX_ARCHIVE_BYTES { return Err(GitWindowsArchiveError::Limit("archive exceeds the download bound".to_owned())); }
    let tail_len = file_len.min(65_557) as usize;
    let tail_offset = file_len - tail_len as u64;
    file.seek(SeekFrom::Start(tail_offset)).map_err(|source| io_error("seek MinGit ZIP trailer", Path::new(MINGIT_ASSET), source))?;
    let mut tail = vec![0_u8; tail_len];
    file.read_exact(&mut tail).map_err(|source| io_error("read MinGit ZIP trailer", Path::new(MINGIT_ASSET), source))?;
    let eocd_index = tail.windows(4).rposition(|bytes| bytes == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| GitWindowsArchiveError::Archive("end-of-central-directory record is missing".to_owned()))?;
    if tail.len() < eocd_index + 22 { return Err(GitWindowsArchiveError::Archive("truncated end-of-central-directory record".to_owned())); }
    let eocd = &tail[eocd_index..];
    if le_u16(eocd, 4) != 0 || le_u16(eocd, 6) != 0 || le_u16(eocd, 8) != le_u16(eocd, 10) {
        return Err(GitWindowsArchiveError::Archive("multi-disk ZIP archives are unsupported".to_owned()));
    }
    let count = le_u16(eocd, 10) as usize;
    let central_bytes = le_u32(eocd, 12) as u64;
    let central_offset = le_u32(eocd, 16) as u64;
    if count == u16::MAX as usize || central_bytes == u32::MAX as u64 || central_offset == u32::MAX as u64 {
        return Err(GitWindowsArchiveError::Archive("ZIP64 archives are unsupported by this bounded inspector".to_owned()));
    }
    if count > MAX_ENTRIES || central_offset.checked_add(central_bytes).is_none_or(|end| end > tail_offset + eocd_index as u64) {
        return Err(GitWindowsArchiveError::Archive("central directory bounds are invalid".to_owned()));
    }
    file.seek(SeekFrom::Start(central_offset)).map_err(|source| io_error("seek MinGit central directory", Path::new(MINGIT_ASSET), source))?;
    let mut entries = Vec::with_capacity(count);
    let mut consumed = 0_u64;
    for _ in 0..count {
        let mut header = [0_u8; 46];
        file.read_exact(&mut header).map_err(|source| io_error("read MinGit central directory", Path::new(MINGIT_ASSET), source))?;
        if le_u32(&header, 0) != 0x0201_4b50 { return Err(GitWindowsArchiveError::Archive("invalid central-directory entry signature".to_owned())); }
        let flags = le_u16(&header, 8);
        if flags & 1 != 0 { return Err(GitWindowsArchiveError::Archive("encrypted ZIP entries are unsupported".to_owned())); }
        let name_len = le_u16(&header, 28) as usize;
        let extra_len = le_u16(&header, 30) as usize;
        let comment_len = le_u16(&header, 32) as usize;
        let variable_len = name_len.checked_add(extra_len).and_then(|n| n.checked_add(comment_len))
            .ok_or_else(|| GitWindowsArchiveError::Limit("central-directory entry length overflowed".to_owned()))?;
        consumed = consumed.checked_add(46 + variable_len as u64)
            .ok_or_else(|| GitWindowsArchiveError::Limit("central-directory size overflowed".to_owned()))?;
        if consumed > central_bytes { return Err(GitWindowsArchiveError::Archive("central-directory entry exceeds declared bounds".to_owned())); }
        let mut variable = vec![0_u8; variable_len];
        file.read_exact(&mut variable).map_err(|source| io_error("read MinGit central-directory name", Path::new(MINGIT_ASSET), source))?;
        let name = std::str::from_utf8(&variable[..name_len])
            .map_err(|_| GitWindowsArchiveError::Archive("archive path is not valid UTF-8".to_owned()))?.to_owned();
        let compressed_size = le_u32(&header, 20);
        let uncompressed_size = le_u32(&header, 24);
        let local_header_offset = le_u32(&header, 42);
        if compressed_size == u32::MAX || uncompressed_size == u32::MAX || local_header_offset == u32::MAX {
            return Err(GitWindowsArchiveError::Archive(format!("ZIP64 entry is unsupported: {name}")));
        }
        entries.push(CentralEntry {
            is_directory: name.ends_with('/'),
            name,
            compression_method: le_u16(&header, 10),
            crc32: le_u32(&header, 16),
            compressed_size: compressed_size as u64,
            uncompressed_size: uncompressed_size as u64,
            local_header_offset: local_header_offset as u64,
        });
    }
    if consumed != central_bytes { return Err(GitWindowsArchiveError::Archive("central-directory size does not match entry records".to_owned())); }
    Ok(entries)
}

fn le_u16(bytes: &[u8], offset: usize) -> u16 { u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) }
fn le_u32(bytes: &[u8], offset: usize) -> u32 { u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) }

fn acquire_archive(destination: &Path) -> Result<u64, GitWindowsArchiveError> {
    if destination.is_file() {
        let (size, digest) = hash_file(destination)?;
        if digest.eq_ignore_ascii_case(MINGIT_SHA256) && size <= MAX_ARCHIVE_BYTES {
            return Ok(size);
        }
        fs::remove_file(destination).map_err(|source| io_error("remove corrupt cached MinGit archive", destination, source))?;
    }
    let parent = destination.parent().ok_or_else(|| GitWindowsArchiveError::Metadata("archive cache path has no parent".to_owned()))?;
    let temporary = parent.join(format!(".{MINGIT_ASSET}.download-{}", std::process::id()));
    let result = (|| {
        let client = reqwest::blocking::Client::builder()
            .user_agent("Vapor managed Git archive inventory")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build().map_err(GitWindowsArchiveError::Http)?;
        let response = client.get(MINGIT_URL).send()
            .map_err(GitWindowsArchiveError::Http)?
            .error_for_status().map_err(GitWindowsArchiveError::Http)?;
        if response.content_length().is_some_and(|size| size > MAX_ARCHIVE_BYTES) {
            return Err(GitWindowsArchiveError::Limit(format!("MinGit exceeds {MAX_ARCHIVE_BYTES} bytes")));
        }
        let mut input = response.take(MAX_ARCHIVE_BYTES + 1);
        let mut output = OpenOptions::new().write(true).create_new(true).open(&temporary)
            .map_err(|source| io_error("create temporary MinGit archive", &temporary, source))?;
        let mut context = Context::new(&SHA256);
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = input.read(&mut buffer)
                .map_err(|source| io_error("download MinGit archive", &temporary, source))?;
            if count == 0 { break; }
            total += count as u64;
            if total > MAX_ARCHIVE_BYTES {
                return Err(GitWindowsArchiveError::Limit(format!("MinGit exceeds {MAX_ARCHIVE_BYTES} bytes")));
            }
            context.update(&buffer[..count]);
            output.write_all(&buffer[..count])
                .map_err(|source| io_error("write MinGit archive", &temporary, source))?;
        }
        output.sync_all().map_err(|source| io_error("sync MinGit archive", &temporary, source))?;
        let actual = hex(context.finish().as_ref());
        if !actual.eq_ignore_ascii_case(MINGIT_SHA256) {
            return Err(GitWindowsArchiveError::DigestMismatch { expected: MINGIT_SHA256.to_owned(), actual });
        }
        fs::rename(&temporary, destination)
            .map_err(|source| io_error("promote verified MinGit archive", destination, source))?;
        Ok(total)
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

fn validate_archive_path(name: &str) -> Result<(), GitWindowsArchiveError> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return Err(GitWindowsArchiveError::UnsafePath(name.to_owned()));
    }
    let normalized = name.strip_suffix('/').unwrap_or(name);
    if normalized.is_empty() || Path::new(normalized).components().any(|part| {
        !matches!(part, Component::Normal(_))
    }) {
        return Err(GitWindowsArchiveError::UnsafePath(name.to_owned()));
    }
    Ok(())
}

fn is_license_or_notice(path: &str) -> bool {
    path.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        part.contains("license") || part.contains("copying") || part.contains("copyright")
            || part.contains("notice") || part == "authors" || part == "credits"
    })
}

fn sha256_file(path: &Path) -> Result<String, GitWindowsArchiveError> {
    hash_file(path).map(|(_, digest)| digest)
}

fn hash_file(path: &Path) -> Result<(u64, String), GitWindowsArchiveError> {
    let mut file = File::open(path).map_err(|source| io_error("open cached MinGit archive", path, source))?;
    let mut context = Context::new(&SHA256);
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|source| io_error("hash cached MinGit archive", path, source))?;
        if count == 0 { break; }
        total += count as u64;
        context.update(&buffer[..count]);
    }
    Ok((total, hex(context.finish().as_ref())))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes { output.push(HEX[(byte >> 4) as usize] as char); output.push(HEX[(byte & 0xf) as usize] as char); }
    output
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> GitWindowsArchiveError {
    GitWindowsArchiveError::Io { action, path: path.to_path_buf(), source }
}

#[derive(Debug)]
pub enum GitWindowsArchiveError {
    Http(reqwest::Error),
    Io { action: &'static str, path: PathBuf, source: io::Error },
    Archive(String),
    UnsafePath(String),
    Limit(String),
    Metadata(String),
    DigestMismatch { expected: String, actual: String },
}

impl std::fmt::Display for GitWindowsArchiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => write!(formatter, "MinGit download failed: {error}"),
            Self::Io { action, path, source } => write!(formatter, "could not {action} at {}: {source}", path.display()),
            Self::Archive(error) => write!(formatter, "MinGit ZIP inspection failed: {error}"),
            Self::UnsafePath(path) => write!(formatter, "MinGit contains an unsafe or duplicate archive path: {path}"),
            Self::Limit(error) => write!(formatter, "MinGit inventory limit exceeded: {error}"),
            Self::Metadata(error) => write!(formatter, "MinGit inventory metadata failed: {error}"),
            Self::DigestMismatch { expected, actual } => write!(formatter, "MinGit archive SHA-256 mismatch: expected {expected}, got {actual}"),
        }
    }
}

impl std::error::Error for GitWindowsArchiveError {}
