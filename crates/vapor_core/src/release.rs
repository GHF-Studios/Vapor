//! Verification primitives for immutable Vapor application releases.
//!
//! Trust is pinned into the installed updater, never supplied by the downloaded
//! manifest. An unknown key ID fails closed.

use ring::{digest, signature};
use semver::Version;
use serde::Deserialize;
use std::io::Read;

const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_GIT_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_GIT_SOURCE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_GIT_NOTICES_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIGNATURE_HEX_BYTES: usize = 128;
pub const RELEASE_ORIGIN: &str = "https://vapor.ghf-studios.site";

/// Offline-pinned application release keys. The private signing key is held
/// separately by the release operator and must never be placed in this tree
/// or on the public release host.
pub const TRUSTED_RELEASE_KEYS: &[(&str, [u8; 32])] = &[(
    "vapor-release-2026-11-24",
    [
        0x1e, 0x3a, 0xb2, 0x2c, 0x2c, 0xf6, 0xcb, 0x7c, 0x67, 0xcf, 0x92, 0xca, 0x5a, 0x9e,
        0x24, 0x2e, 0x8f, 0xc6, 0x97, 0xc1, 0x5e, 0xd0, 0x45, 0x89, 0xda, 0x4c, 0xbd, 0x7e,
        0xd1, 0x84, 0x2a, 0x43,
    ],
)];

/// A release manifest after its exact response bytes have been authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedReleaseManifest {
    pub key_id: String,
    pub version: Version,
    pub channel: String,
    pub platform: String,
    pub minimum_updater_version: Version,
    pub artifact: VerifiedArtifact,
    pub setup: VerifiedArtifact,
    pub managed_git: Option<VerifiedManagedGitRelease>,
    pub release_notes_url: String,
}

/// The archive identity signed by the release manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// Optional Developer-only Git payload authenticated by the app release signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManagedGitRelease {
    pub version: Version,
    pub platform: String,
    pub artifact: VerifiedArtifact,
    pub source: VerifiedArtifact,
    pub notices: VerifiedArtifact,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    schema: u32,
    key_id: String,
    version: String,
    channel: String,
    platform: String,
    minimum_updater_version: String,
    artifact: ReleaseArtifact,
    setup: ReleaseArtifact,
    #[serde(default)]
    managed_git: Option<ManagedGitRelease>,
    release_notes_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArtifact {
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedGitRelease {
    version: String,
    platform: String,
    artifact: ReleaseArtifact,
    source: ReleaseArtifact,
    notices: ReleaseArtifact,
}

/// Authenticate and validate the exact manifest bytes returned by Artifact.
///
/// The signed `key_id` must resolve to this build's offline-pinned raw Ed25519
/// public key set. `signature_hex` is the detached lowercase hexadecimal
/// Ed25519 signature over `manifest_bytes` without parsing or re-serialization.
pub fn verify_manifest(
    manifest_bytes: &[u8],
    signature_hex: &str,
    expected_channel: &str,
    expected_platform: &str,
    updater_version: &Version,
) -> Result<VerifiedReleaseManifest, ReleaseVerificationError> {
    if manifest_bytes.is_empty() || manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(error("manifest is empty or exceeds 64 KiB"));
    }
    if signature_hex.len() != MAX_SIGNATURE_HEX_BYTES
        || !signature_hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        || signature_hex.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(error("detached signature is not 64-byte lowercase hex"));
    }

    let manifest: ReleaseManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|_| error("manifest JSON does not match the release schema"))?;
    if !matches!(manifest.schema, 1 | 2) {
        return Err(error("unsupported release manifest schema"));
    }
    if (manifest.schema == 1 && manifest.managed_git.is_some())
        || (manifest.schema == 2 && manifest.managed_git.is_none())
    {
        return Err(error("managed Git metadata does not match the release schema"));
    }
    if !valid_key_id(&manifest.key_id) {
        return Err(error("manifest key_id is invalid"));
    }
    let mut matching_keys = TRUSTED_RELEASE_KEYS
        .iter()
        .filter(|(key_id, _)| *key_id == manifest.key_id);
    let public_key = matching_keys
        .next()
        .map(|(_, key)| key)
        .ok_or_else(|| error("manifest key_id is not in the trusted release keyring"))?;
    if matching_keys.next().is_some() {
        return Err(error("trusted release keyring contains a duplicate key_id"));
    }
    let signature_bytes = decode_hex(signature_hex)?;
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(manifest_bytes, &signature_bytes)
        .map_err(|_| error("manifest signature verification failed"))?;

    let version = Version::parse(&manifest.version)
        .map_err(|_| error("manifest version is not valid SemVer"))?;
    let minimum_updater_version = Version::parse(&manifest.minimum_updater_version)
        .map_err(|_| error("minimum_updater_version is not valid SemVer"))?;
    if minimum_updater_version > updater_version.clone() {
        return Err(error("release requires a newer Vapor updater"));
    }
    if manifest.channel != expected_channel {
        return Err(error("release channel does not match the requested channel"));
    }
    if manifest.platform != expected_platform {
        return Err(error("release platform does not match this installation"));
    }

    // Keep the path construction explicit and canonical: arbitrary URLs in a
    // signed manifest must never redirect the bootstrapper to another origin.
    let expected_artifact_url = format!(
        "{RELEASE_ORIGIN}/api/updates/v1/vapor/releases/{version}/{}/vapor.tar.gz",
        manifest.platform
    );
    if manifest.artifact.url != expected_artifact_url {
        return Err(error("artifact URL is not the canonical immutable release URL"));
    }
    let setup_name = if manifest.platform == "windows-x86_64" {
        "vapor-setup.exe"
    } else {
        "vapor-setup"
    };
    let expected_setup_url = format!(
        "{RELEASE_ORIGIN}/api/updates/v1/vapor/releases/{version}/{}/{setup_name}",
        manifest.platform
    );
    if manifest.setup.url != expected_setup_url {
        return Err(error("setup URL is not the canonical immutable release URL"));
    }
    if manifest.release_notes_url != RELEASE_ORIGIN
        && !manifest
            .release_notes_url
            .starts_with(&format!("{RELEASE_ORIGIN}/"))
    {
        return Err(error("release notes URL is outside the Vapor HTTPS origin"));
    }
    if manifest.artifact.size == 0 || manifest.artifact.size > MAX_ARTIFACT_BYTES {
        return Err(error("artifact size is empty or exceeds the 8 GiB limit"));
    }
    if manifest.artifact.sha256.len() != 64
        || !manifest
            .artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error("artifact SHA-256 is not 64-byte lowercase hex"));
    }
    if manifest.setup.size == 0
        || manifest.setup.size > 512 * 1024 * 1024
        || manifest.setup.sha256.len() != 64
        || !manifest
            .setup
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error("setup size or SHA-256 is invalid"));
    }

    let managed_git = manifest
        .managed_git
        .map(|git| validate_managed_git(git, &version, &manifest.platform))
        .transpose()?;

    Ok(VerifiedReleaseManifest {
        key_id: manifest.key_id,
        version,
        channel: manifest.channel,
        platform: manifest.platform,
        minimum_updater_version,
        artifact: VerifiedArtifact {
            url: manifest.artifact.url,
            size: manifest.artifact.size,
            sha256: manifest.artifact.sha256,
        },
        setup: VerifiedArtifact {
            url: manifest.setup.url,
            size: manifest.setup.size,
            sha256: manifest.setup.sha256,
        },
        managed_git,
        release_notes_url: manifest.release_notes_url,
    })
}

fn validate_managed_git(
    git: ManagedGitRelease,
    release_version: &Version,
    release_platform: &str,
) -> Result<VerifiedManagedGitRelease, ReleaseVerificationError> {
    let version = Version::parse(&git.version)
        .map_err(|_| error("managed Git version is not valid SemVer"))?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(error("managed Git version must be a stable SemVer release"));
    }
    if git.platform != release_platform {
        return Err(error("managed Git platform does not match the release platform"));
    }
    let base = format!(
        "{RELEASE_ORIGIN}/api/updates/v1/vapor/releases/{release_version}/{release_platform}"
    );
    validate_managed_git_artifact(
        &git.artifact,
        &format!("{base}/git.tar.gz"),
        MAX_GIT_ARTIFACT_BYTES,
    )?;
    validate_managed_git_artifact(
        &git.source,
        &format!("{base}/git-source.tar.gz"),
        MAX_GIT_SOURCE_BYTES,
    )?;
    validate_managed_git_artifact(
        &git.notices,
        &format!("{base}/git-notices.tar.gz"),
        MAX_GIT_NOTICES_BYTES,
    )?;
    Ok(VerifiedManagedGitRelease {
        version,
        platform: git.platform,
        artifact: into_verified_artifact(git.artifact),
        source: into_verified_artifact(git.source),
        notices: into_verified_artifact(git.notices),
    })
}

fn validate_managed_git_artifact(
    artifact: &ReleaseArtifact,
    expected_url: &str,
    maximum_size: u64,
) -> Result<(), ReleaseVerificationError> {
    if artifact.url != expected_url {
        return Err(error("managed Git asset URL is not canonical"));
    }
    if artifact.size == 0 || artifact.size > maximum_size {
        return Err(error("managed Git asset size is empty or exceeds its limit"));
    }
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error("managed Git asset SHA-256 is invalid"));
    }
    Ok(())
}

fn into_verified_artifact(artifact: ReleaseArtifact) -> VerifiedArtifact {
    VerifiedArtifact {
        url: artifact.url,
        size: artifact.size,
        sha256: artifact.sha256,
    }
}

/// Stream-check an archive against its authenticated size and digest.
pub fn verify_artifact<R: Read>(
    mut reader: R,
    artifact: &VerifiedArtifact,
) -> Result<(), ReleaseVerificationError> {
    let mut context = digest::Context::new(&digest::SHA256);
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| error("could not read downloaded release archive"))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|size| *size <= artifact.size && *size <= MAX_ARTIFACT_BYTES)
            .ok_or_else(|| error("downloaded release archive exceeds its signed size"))?;
        context.update(&buffer[..count]);
    }
    if total != artifact.size {
        return Err(error("downloaded release archive size does not match the manifest"));
    }
    let actual = encode_hex(context.finish().as_ref());
    if actual != artifact.sha256 {
        return Err(error("downloaded release archive SHA-256 does not match the manifest"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseVerificationError(String);

impl std::fmt::Display for ReleaseVerificationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReleaseVerificationError {}

fn error(message: &str) -> ReleaseVerificationError {
    ReleaseVerificationError(message.to_owned())
}

fn valid_key_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ReleaseVerificationError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_digit(pair[0]).ok_or_else(|| error("signature contains invalid hex"))?;
        let low = hex_digit(pair[1]).ok_or_else(|| error("signature contains invalid hex"))?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}
