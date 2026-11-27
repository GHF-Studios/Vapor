//! Reproducible-input shadow build of upstream Git for Linux.
//!
//! Git, curl, OpenSSL, and zlib are built from pinned source archives with
//! Vapor-managed Zig for the static musl target. The resulting candidate is
//! still kept in the disposable build cache: redistribution eligibility also
//! requires the complete source and license inventory to be reviewed.

use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use crate::{VaporInstallation, cross_target_toolchain::LINUX_GNU_LINKER_VERSION};

const GIT_VERSION: &str = "2.56.1";
const GIT_SOURCE_URL: &str = "https://www.kernel.org/pub/software/scm/git/git-2.56.1.tar.xz";
const GIT_SOURCE_SHA256: &str =
    "f0b0bf89264b9bd2e1c640363e7529643989da76281600473c50745cd34a607f";
const CURL_VERSION: &str = "8.23.0";
const CURL_SOURCE_URL: &str = "https://curl.se/download/curl-8.23.0.tar.xz";
const CURL_SOURCE_SHA256: &str =
    "8dadd7d2536a314f982ad05a651b713f426bf7d02cc2c76cffb9796f2218e6c5";
const OPENSSL_VERSION: &str = "3.5.9";
const OPENSSL_SOURCE_URL: &str = "https://github.com/openssl/openssl/releases/download/openssl-3.5.9/openssl-3.5.9.tar.gz";
const OPENSSL_SOURCE_SHA256: &str =
    "603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a";
const ZLIB_VERSION: &str = "1.4";
const ZLIB_SOURCE_URL: &str = "https://zlib.net/fossils/zlib-1.4.tar.gz";
const ZLIB_SOURCE_SHA256: &str =
    "02dd159783677a749c04adbdd02b55b8a2bcc7e054f51b76b549199a916ad584";
const MAKE_VERSION: &str = "4.4.1";
const MAKE_SOURCE_URL: &str = "https://ftp.gnu.org/gnu/make/make-4.4.1.tar.gz";
const MAKE_SOURCE_SHA256: &str =
    "dd16fb1d67bfab79a72f5e8390735c49e3e8e70b4945a15ab1f81ddb78658fb3";
const RECIPE_REVISION: &str = "2026-11-27.15";
const CACHE_RELATIVE_ROOT: &str = "cache/build/native-tools/git";
const MARKER_NAME: &str = "vapor-git-native-build.toml";
const MAX_SOURCE_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const BUILD_FEATURES: &[&str] = &[
    "https-transport",
    "make-no-nls",
    "make-no-guile",
    "no-expat",
    "no-gettext",
    "no-pcre2",
    "no-perl",
    "no-python",
    "no-rust",
    "no-fuzz-links",
    "no-tcl-tk",
    "zlib",
];
const GIT_TARGET: &str = "x86_64-linux-musl";

#[derive(Debug, Clone)]
pub struct GitNativeBuildReport {
    pub git_version: String,
    pub cache_hit: bool,
    pub cache_key: String,
    pub cache_root: PathBuf,
    pub executable: PathBuf,
    pub runtime_dependencies: BTreeMap<String, String>,
    pub runtime_libraries: BTreeMap<String, String>,
    pub distribution_eligible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitBuildInputs {
    git_source_sha256: String,
    curl_source_sha256: String,
    openssl_source_sha256: String,
    zlib_source_sha256: String,
    make_source_sha256: String,
    recipe_revision: String,
    compiler_identity: String,
    target: String,
    features: Vec<String>,
    runtime_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildMarker {
    schema: u32,
    cache_key: String,
    git_version: String,
    target: String,
    compiler_identity: String,
    recipe_revision: String,
    git_source_sha256: String,
    curl_source_sha256: String,
    openssl_source_sha256: String,
    zlib_source_sha256: String,
    make_source_sha256: String,
    runtime_dependencies: BTreeMap<String, String>,
    runtime_libraries: BTreeMap<String, String>,
    features: Vec<String>,
    inventory_sha256: String,
    distribution_eligible: bool,
}

/// Build Git and its non-system runtime dependencies from pinned upstream
/// sources into Vapor's non-authoritative cache.
pub fn build_linux_shadow(
    installation: &VaporInstallation,
) -> Result<GitNativeBuildReport, GitNativeBuildError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(GitNativeBuildError::UnsupportedHost);
    }

    let data_root = installation.user_data_root();
    let zig_root = data_root.join("tools/zig");
    let zig_path = zig_root.join("zig");
    let zig_marker_path = zig_root.join("vapor-toolchain.toml");
    let zig_marker = fs::read_to_string(&zig_marker_path).map_err(|source| {
        io_error("read managed Zig marker", &zig_marker_path, source)
    })?;
    if !zig_marker.lines().any(|line| line.trim() == "schema = 1")
        || !zig_marker.lines().any(|line| {
            line.trim() == format!("version = \"{LINUX_GNU_LINKER_VERSION}\"")
        })
        || !zig_path.is_file()
    {
        return Err(GitNativeBuildError::ManagedCompilerUnavailable(zig_root));
    }
    let compiler_version = capture(&zig_path, ["version"], None, &[])?;
    let compiler_identity = format!(
        "zig={};mode=static-musl;marker-sha256={}",
        compiler_version.trim(),
        sha256_bytes(zig_marker.as_bytes())
    );
    // Build the disposable Make executable for this host, then use the same
    // managed compiler for a static musl Git payload independent of host C
    // library versions.
    let compiler_path = prepend_path(zig_root.as_path())?;

    let runtime_dependencies = BTreeMap::from([
        ("curl".to_owned(), CURL_VERSION.to_owned()),
        ("openssl".to_owned(), OPENSSL_VERSION.to_owned()),
        ("zlib".to_owned(), ZLIB_VERSION.to_owned()),
        ("linkage".to_owned(), "static musl".to_owned()),
    ]);
    let inputs = GitBuildInputs {
        git_source_sha256: GIT_SOURCE_SHA256.to_owned(),
        curl_source_sha256: CURL_SOURCE_SHA256.to_owned(),
        openssl_source_sha256: OPENSSL_SOURCE_SHA256.to_owned(),
        zlib_source_sha256: ZLIB_SOURCE_SHA256.to_owned(),
        make_source_sha256: MAKE_SOURCE_SHA256.to_owned(),
        recipe_revision: RECIPE_REVISION.to_owned(),
        compiler_identity: compiler_identity.clone(),
        target: GIT_TARGET.to_owned(),
        features: BUILD_FEATURES.iter().map(|feature| (*feature).to_owned()).collect(),
        runtime_dependencies: runtime_dependencies.clone(),
    };
    let cache_key = sha256_bytes(
        &serde_json::to_vec(&inputs).map_err(|error| {
            GitNativeBuildError::Metadata(format!("could not encode cache inputs: {error}"))
        })?,
    );
    let cache_root = data_root.join(CACHE_RELATIVE_ROOT);
    let entry_root = cache_root.join(&cache_key);
    let install_root = entry_root.join("install");
    let git_executable = install_root.join("bin/git");

    if let Some(marker) = validate_cache_entry(
        &entry_root,
        &cache_key,
        &git_executable,
        &compiler_identity,
        &runtime_dependencies,
    )? {
        return Ok(GitNativeBuildReport {
            git_version: marker.git_version,
            cache_hit: true,
            cache_key,
            cache_root: entry_root,
            executable: git_executable,
            runtime_dependencies,
            runtime_libraries: marker.runtime_libraries,
            distribution_eligible: false,
        });
    }

    let lock_path = entry_root.join(".building");
    fs::create_dir_all(&entry_root)
        .map_err(|source| io_error("create Git build cache entry", &entry_root, source))?;
    fs::create_dir(&lock_path).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            GitNativeBuildError::BuildAlreadyRunning(entry_root.clone())
        } else {
            io_error("lock Git build cache entry", &lock_path, source)
        }
    })?;
    let _lock = BuildLock(lock_path.clone());

    // Recheck after taking the lock in case another process completed while we
    // were waiting to acquire it.
    if let Some(marker) = validate_cache_entry(
        &entry_root,
        &cache_key,
        &git_executable,
        &compiler_identity,
        &runtime_dependencies,
    )? {
        return Ok(GitNativeBuildReport {
            git_version: marker.git_version,
            cache_hit: true,
            cache_key,
            cache_root: entry_root,
            executable: git_executable,
            runtime_dependencies,
            runtime_libraries: marker.runtime_libraries,
            distribution_eligible: false,
        });
    }

    let sources_root = cache_root.join("sources");
    fs::create_dir_all(&sources_root)
        .map_err(|source| io_error("create Git source cache", &sources_root, source))?;
    let make_archive = sources_root.join(format!("make-{MAKE_VERSION}.tar.gz"));
    let git_archive = sources_root.join(format!("git-{GIT_VERSION}.tar.xz"));
    let curl_archive = sources_root.join(format!("curl-{CURL_VERSION}.tar.xz"));
    let openssl_archive = sources_root.join(format!("openssl-{OPENSSL_VERSION}.tar.gz"));
    let zlib_archive = sources_root.join(format!("zlib-{ZLIB_VERSION}.tar.gz"));

    let marker = build_git_cache_entry(
        &entry_root,
        &install_root,
        &make_archive,
        &git_archive,
        &curl_archive,
        &openssl_archive,
        &zlib_archive,
        &compiler_identity,
        &compiler_path,
        &runtime_dependencies,
        &cache_key,
    )?;
    Ok(GitNativeBuildReport {
        git_version: marker.git_version,
        cache_hit: false,
        cache_key,
        cache_root: entry_root,
        executable: git_executable,
        runtime_dependencies,
        runtime_libraries: marker.runtime_libraries,
        distribution_eligible: marker.distribution_eligible,
    })
}

fn build_git_cache_entry(
    entry_root: &Path,
    install_root: &Path,
    make_archive: &Path,
    git_archive: &Path,
    curl_archive: &Path,
    openssl_archive: &Path,
    zlib_archive: &Path,
    compiler_identity: &str,
    compiler_path: &str,
    runtime_dependencies: &BTreeMap<String, String>,
    cache_key: &str,
) -> Result<BuildMarker, GitNativeBuildError> {
    let result = (|| {
        acquire_source(make_archive, MAKE_SOURCE_URL, MAKE_SOURCE_SHA256)?;
        acquire_source(git_archive, GIT_SOURCE_URL, GIT_SOURCE_SHA256)?;
        acquire_source(curl_archive, CURL_SOURCE_URL, CURL_SOURCE_SHA256)?;
        acquire_source(openssl_archive, OPENSSL_SOURCE_URL, OPENSSL_SOURCE_SHA256)?;
        acquire_source(zlib_archive, ZLIB_SOURCE_URL, ZLIB_SOURCE_SHA256)?;

        let build_root = entry_root.join("build");
        let make_source = build_root.join("make-source");
        let git_source = build_root.join("git-source");
        let make_executable = make_source.join("make");
        let make_prefix = entry_root.join("make-install");
        let staged_install = entry_root.join("install.staging");
        let staged_prefix = staged_install.join(
            install_root.strip_prefix("/").map_err(|_| {
                GitNativeBuildError::Metadata(format!(
                    "Git install prefix must be absolute: {}",
                    install_root.display()
                ))
            })?,
        );
        let marker_path = entry_root.join(MARKER_NAME);
        remove_path(&staged_install)?;
        remove_path(&marker_path)?;
        fs::create_dir_all(&build_root)
            .map_err(|source| io_error("create Git source build directory", &build_root, source))?;

        build_bootstrap_make(
            make_archive,
            &make_source,
            &make_executable,
            &make_prefix,
            compiler_path,
            "-O2 -g0",
        )?;
        let dependency_prefix = entry_root.join("dependencies");
        build_static_dependencies(
            &build_root,
            &dependency_prefix,
            &make_executable,
            compiler_path,
            curl_archive,
            openssl_archive,
            zlib_archive,
        )?;
        let target_cflags = format!("-O2 -g0 -target {GIT_TARGET}");
        let target_ldflags = format!(
            "-target {GIT_TARGET} -static -L{}/lib64 -L{}/lib",
            dependency_prefix.display(),
            dependency_prefix.display(),
        );
        let target_libs = "-lcurl -lssl -lcrypto -lz -ldl -pthread";
        let pkg_config_libdir = env::join_paths([
            dependency_prefix.join("lib/pkgconfig"),
            dependency_prefix.join("lib64/pkgconfig"),
        ])
        .map_err(|error| GitNativeBuildError::Metadata(format!("could not construct Git pkg-config path: {error}")))?
        .to_string_lossy()
        .into_owned();
        build_and_stage_git(
            git_archive,
            &git_source,
            &make_executable,
            &dependency_prefix,
            &install_root,
            &staged_install,
            &staged_prefix,
            compiler_path,
            &pkg_config_libdir,
            &target_cflags,
            &target_ldflags,
            target_libs,
        )?;

        remove_path(install_root)?;
        fs::rename(&staged_prefix, install_root)
            .map_err(|source| io_error("promote Git into its shadow cache", install_root, source))?;
        remove_path(&staged_install)?;
        let runtime_libraries = runtime_library_inventory(install_root)?;
        require_static_runtime(&runtime_libraries)?;
        let inventory_sha256 = inventory_digest(install_root)?;
        let marker = BuildMarker {
            schema: 3,
            cache_key: cache_key.to_owned(),
            git_version: format!("git version {GIT_VERSION}"),
            target: GIT_TARGET.to_owned(),
            compiler_identity: compiler_identity.to_owned(),
            recipe_revision: RECIPE_REVISION.to_owned(),
            git_source_sha256: GIT_SOURCE_SHA256.to_owned(),
            curl_source_sha256: CURL_SOURCE_SHA256.to_owned(),
            openssl_source_sha256: OPENSSL_SOURCE_SHA256.to_owned(),
            zlib_source_sha256: ZLIB_SOURCE_SHA256.to_owned(),
            make_source_sha256: MAKE_SOURCE_SHA256.to_owned(),
            runtime_dependencies: runtime_dependencies.clone(),
            runtime_libraries,
            features: BUILD_FEATURES.iter().map(|feature| (*feature).to_owned()).collect(),
            inventory_sha256,
            distribution_eligible: false,
        };
        write_marker(entry_root, &marker)?;
        Ok(marker)
    })();

    if result.is_err() {
        let _ = remove_path(install_root);
        let _ = remove_path(&entry_root.join("install.staging"));
        let _ = remove_path(&entry_root.join(MARKER_NAME));
        let _ = remove_path(&entry_root.join("dependencies"));
    }
    result
}

fn build_static_dependencies(
    build_root: &Path,
    prefix: &Path,
    make: &Path,
    compiler_path: &str,
    curl_archive: &Path,
    openssl_archive: &Path,
    zlib_archive: &Path,
) -> Result<(), GitNativeBuildError> {
    let zlib_source = build_root.join("zlib-source");
    let openssl_source = build_root.join("openssl-source");
    let curl_source = build_root.join("curl-source");
    extract_source(zlib_archive, &zlib_source, "zlib-1.4", false)?;
    extract_source(openssl_archive, &openssl_source, "openssl-3.5.9", false)?;
    extract_source(curl_archive, &curl_source, "curl-8.23.0", true)?;

    fs::create_dir_all(prefix)
        .map_err(|source| io_error("create static Git dependency prefix", prefix, source))?;
    let target_cc = format!("zig cc -target {GIT_TARGET}");
    let target_ar = "zig ar";
    let target_ranlib = "zig ranlib";
    let target_cflags = format!("-O2 -g0 -target {GIT_TARGET} -fPIC");
    let target_ldflags = format!(
        "-target {GIT_TARGET} -static -L{}/lib64 -L{}/lib",
        prefix.display(),
        prefix.display(),
    );
    let base_environment = [
        ("CC", target_cc.as_str()),
        ("AR", target_ar),
        ("RANLIB", target_ranlib),
        ("CFLAGS", target_cflags.as_str()),
        ("LDFLAGS", target_ldflags.as_str()),
        ("PATH", compiler_path),
    ];

    let prefix_arg = prefix.to_string_lossy().into_owned();
    let zlib_configure = run_output(
        "configure pinned static zlib",
        zlib_source.join("configure"),
        ["--static", "--prefix", prefix_arg.as_str()],
        Some(&zlib_source),
        &base_environment,
    )?;
    if !zlib_configure.status.success() {
        return Err(command_failed("configure pinned static zlib", zlib_configure));
    }
    build_and_install(
        "build pinned static zlib",
        make,
        &zlib_source,
        &["-j", "2"],
        &["install"],
        &base_environment,
    )?;

    let openssl_prefix = format!("--prefix={prefix_arg}");
    let openssl_dir = "--openssldir=/etc/ssl";
    let openssl_configure = run_output(
        "configure pinned static OpenSSL",
        openssl_source.join("Configure"),
        [
            "linux-x86_64",
            "no-shared",
            "no-asm",
            "no-tests",
            "no-module",
            "no-dso",
            openssl_prefix.as_str(),
            openssl_dir,
        ],
        Some(&openssl_source),
        &base_environment,
    )?;
    if !openssl_configure.status.success() {
        return Err(command_failed("configure pinned static OpenSSL", openssl_configure));
    }
    build_and_install(
        "build pinned static OpenSSL",
        make,
        &openssl_source,
        &["-j", "2"],
        &["install_sw"],
        &base_environment,
    )?;

    let pkg_config_libdir = env::join_paths([prefix.join("lib/pkgconfig"), prefix.join("lib64/pkgconfig")])
        .map_err(|error| GitNativeBuildError::Metadata(format!("could not construct static dependency pkg-config path: {error}")))?
        .to_string_lossy()
        .into_owned();
    let cppflags = format!("-I{}/include", prefix.display());
    let curl_environment = [
        ("CC", target_cc.as_str()),
        ("AR", target_ar),
        ("RANLIB", target_ranlib),
        ("CFLAGS", target_cflags.as_str()),
        ("CPPFLAGS", cppflags.as_str()),
        ("LDFLAGS", target_ldflags.as_str()),
        ("PKG_CONFIG_LIBDIR", pkg_config_libdir.as_str()),
        ("PATH", compiler_path),
    ];
    let curl_host = GIT_TARGET;
    let with_openssl = format!("--with-openssl={prefix_arg}");
    let with_zlib = format!("--with-zlib={prefix_arg}");
    let curl_configure = run_output(
        "configure pinned static libcurl",
        curl_source.join("configure"),
        [
            format!("--host={curl_host}"),
            format!("--prefix={prefix_arg}"),
            "--disable-shared".to_owned(),
            "--enable-static".to_owned(),
            with_openssl,
            with_zlib,
            "--without-brotli".to_owned(),
            "--without-zstd".to_owned(),
            "--without-nghttp2".to_owned(),
            "--without-nghttp3".to_owned(),
            "--without-ngtcp2".to_owned(),
            "--without-libidn2".to_owned(),
            "--without-libpsl".to_owned(),
            "--without-libssh2".to_owned(),
            "--without-gssapi".to_owned(),
            "--without-libgsasl".to_owned(),
            "--disable-ftp".to_owned(),
            "--disable-file".to_owned(),
            "--disable-ldap".to_owned(),
            "--disable-ldaps".to_owned(),
            "--disable-rtsp".to_owned(),
            "--disable-dict".to_owned(),
            "--disable-telnet".to_owned(),
            "--disable-tftp".to_owned(),
            "--disable-pop3".to_owned(),
            "--disable-imap".to_owned(),
            "--disable-mqtt".to_owned(),
        ],
        Some(&curl_source),
        &curl_environment,
    )?;
    if !curl_configure.status.success() {
        return Err(command_failed("configure pinned static libcurl", curl_configure));
    }
    build_and_install(
        "build pinned static libcurl",
        make,
        &curl_source,
        &["-j", "2"],
        &["install"],
        &curl_environment,
    )?;

    for path in [
        prefix.join("lib/libz.a"),
        prefix.join("lib64/libssl.a"),
        prefix.join("lib64/libcrypto.a"),
        prefix.join("lib/libcurl.a"),
    ] {
        if !path.is_file() {
            return Err(GitNativeBuildError::MissingStaticDependency(path));
        }
    }
    Ok(())
}

fn build_and_install(
    operation: &'static str,
    make: &Path,
    source: &Path,
    build_args: &[&str],
    install_args: &[&str],
    environment: &[(&str, &str)],
) -> Result<(), GitNativeBuildError> {
    let build = run_output(operation, make, build_args.iter().copied(), Some(source), environment)?;
    if !build.status.success() {
        return Err(command_failed(operation, build));
    }
    let install_operation = "install pinned static Git dependency";
    let install = run_output(install_operation, make, install_args.iter().copied(), Some(source), environment)?;
    if !install.status.success() {
        return Err(command_failed(install_operation, install));
    }
    Ok(())
}

fn build_bootstrap_make(
    archive: &Path,
    source_root: &Path,
    executable: &Path,
    prefix: &Path,
    compiler_path: &str,
    cflags: &str,
) -> Result<(), GitNativeBuildError> {
    extract_source(archive, source_root, "make-4.4.1", false)?;
    let prefix = prefix.to_string_lossy().into_owned();
    let environment = [("CC", "zig cc"), ("CFLAGS", cflags), ("PATH", compiler_path)];
    let configure = run_output(
        "configure pinned GNU Make bootstrap",
        source_root.join("configure"),
        ["--prefix", prefix.as_str(), "--disable-nls", "--without-guile"],
        Some(source_root),
        &environment,
    )?;
    if !configure.status.success() {
        return Err(command_failed("configure pinned GNU Make bootstrap", configure));
    }
    if !program_reports_version(executable, "--version", MAKE_VERSION)? {
        let output = run_output(
            "bootstrap managed GNU Make",
            "sh",
            ["build.sh"],
            Some(source_root),
            &environment,
        )?;
        if !output.status.success() {
            return Err(command_failed("bootstrap managed GNU Make", output));
        }
    }
    if !program_reports_version(executable, "--version", MAKE_VERSION)? {
        return Err(GitNativeBuildError::UnexpectedToolVersion {
            tool: "GNU Make",
            expected: MAKE_VERSION.to_owned(),
            path: executable.to_path_buf(),
        });
    }
    Ok(())
}

fn build_and_stage_git(
    archive: &Path,
    source_root: &Path,
    make: &Path,
    dependency_prefix: &Path,
    install_root: &Path,
    staged_root: &Path,
    staged_prefix: &Path,
    compiler_path: &str,
    pkg_config_libdir: &str,
    cflags: &str,
    ldflags: &str,
    libs: &str,
) -> Result<(), GitNativeBuildError> {
    extract_source(archive, source_root, "git-2.56.1", true)?;
    // Git compiles its exec-path from the configured prefix. Keep that final
    // immutable cache location in the executable while DESTDIR stages the
    // installed tree under the cache entry for atomic promotion.
    let final_prefix = install_root.to_string_lossy().into_owned();
    // Git's configure/build scripts locate libcurl through curl-config. Put
    // the pinned build prefix first so an ambient host curl-config cannot
    // silently select distribution libraries for this static candidate.
    let mut build_paths = vec![dependency_prefix.join("bin")];
    build_paths.extend(env::split_paths(std::ffi::OsStr::new(compiler_path)));
    let build_path = env::join_paths(build_paths)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| GitNativeBuildError::Metadata(format!("could not construct managed Git build PATH: {error}")))?;
    let environment = [
        ("CC", "zig cc -target x86_64-linux-musl"),
        ("CFLAGS", cflags),
        ("LDFLAGS", ldflags),
        ("LIBS", libs),
        ("PKG_CONFIG_LIBDIR", pkg_config_libdir),
        ("PATH", build_path.as_str()),
    ];
    let configure = run_output(
        "configure source Git",
        source_root.join("configure"),
        ["--prefix", final_prefix.as_str()],
        Some(source_root),
        &environment,
    )?;
    if !configure.status.success() {
        return Err(command_failed("configure source Git", configure));
    }

    let clean = run_output(
        "clean source Git objects for the configured final prefix",
        make,
        ["clean"],
        Some(source_root),
        &environment,
    )?;
    if !clean.status.success() {
        return Err(command_failed(
            "clean source Git objects for the configured final prefix",
            clean,
        ));
    }

    let jobs = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .max(1)
        .to_string();
    let feature_args = [
        "NO_RUST=1",
        "NO_PCRE2=YesPlease",
        "NO_PERL=1",
        "NO_TCLTK=1",
        "NO_GETTEXT=1",
        "NO_EXPAT=1",
        "NO_PYTHON=1",
        "LINK_FUZZ_PROGRAMS=",
    ];
    let mut build_args = vec!["-j".to_owned(), jobs];
    build_args.extend(feature_args.iter().map(|feature| (*feature).to_owned()));
    build_args.push("all".to_owned());
    let build = run_output(
        "build source Git with Vapor-managed Zig",
        make,
        build_args.iter().map(String::as_str),
        Some(source_root),
        &environment,
    )?;
    if !build.status.success() {
        return Err(command_failed("build source Git", build));
    }

    let mut install_args = vec![format!("DESTDIR={}", staged_root.display())];
    install_args.extend(feature_args.iter().map(|feature| (*feature).to_owned()));
    install_args.push("install".to_owned());
    let install = run_output(
        "stage source Git in its shadow cache",
        make,
        install_args.iter().map(String::as_str),
        Some(source_root),
        &environment,
    )?;
    if !install.status.success() {
        return Err(command_failed("stage source Git in its shadow cache", install));
    }
    if !is_executable(&staged_prefix.join("bin/git"))
        || !is_executable(&staged_prefix.join("libexec/git-core/git-remote-https"))
    {
        return Err(GitNativeBuildError::IncompleteGitInstall(staged_prefix.to_path_buf()));
    }

    let actual_version = read_git_version(&staged_prefix.join("bin/git"))?;
    let expected_version = format!("git version {GIT_VERSION}");
    if actual_version != expected_version {
        return Err(GitNativeBuildError::UnexpectedGitVersion {
            expected: GIT_VERSION.to_owned(),
            actual: actual_version,
        });
    }
    let actual_exec_path = capture(&staged_prefix.join("bin/git"), ["--exec-path"], None, &[])?;
    let expected_exec_path = install_root.join("libexec/git-core");
    if PathBuf::from(actual_exec_path.trim()) != expected_exec_path {
        return Err(GitNativeBuildError::UnexpectedGitExecPath {
            expected: expected_exec_path,
            actual: actual_exec_path.trim().to_owned(),
        });
    }
    Ok(())
}

fn validate_cache_entry(
    entry_root: &Path,
    expected_key: &str,
    git_executable: &Path,
    expected_compiler_identity: &str,
    expected_runtime_dependencies: &BTreeMap<String, String>,
) -> Result<Option<BuildMarker>, GitNativeBuildError> {
    let marker_path = entry_root.join(MARKER_NAME);
    if !marker_path.is_file() || !git_executable.is_file() {
        return Ok(None);
    }
    let source = fs::read_to_string(&marker_path)
        .map_err(|error| io_error("read Git build cache marker", &marker_path, error))?;
    let marker = match toml::from_str::<BuildMarker>(&source) {
        Ok(marker) => marker,
        Err(_) => return Ok(None),
    };
    if marker.schema != 3
        || marker.cache_key != expected_key
        || marker.git_version != format!("git version {GIT_VERSION}")
        || marker.target != GIT_TARGET
        || marker.git_source_sha256 != GIT_SOURCE_SHA256
        || marker.curl_source_sha256 != CURL_SOURCE_SHA256
        || marker.openssl_source_sha256 != OPENSSL_SOURCE_SHA256
        || marker.zlib_source_sha256 != ZLIB_SOURCE_SHA256
        || marker.make_source_sha256 != MAKE_SOURCE_SHA256
        || marker.recipe_revision != RECIPE_REVISION
        || marker.compiler_identity != expected_compiler_identity
        || marker.runtime_dependencies != *expected_runtime_dependencies
        || marker.features != BUILD_FEATURES.iter().map(|feature| (*feature).to_owned()).collect::<Vec<_>>()
        || marker.distribution_eligible
        || !is_executable(git_executable)
        || !is_executable(&entry_root.join("install/libexec/git-core/git-remote-https"))
    {
        return Ok(None);
    }
    if read_git_version(git_executable)? != marker.git_version
        || runtime_library_inventory(&entry_root.join("install"))? != marker.runtime_libraries
        || inventory_digest(&entry_root.join("install"))? != marker.inventory_sha256
        || !has_static_runtime(&marker.runtime_libraries)
    {
        return Ok(None);
    }
    let expected_exec_path = entry_root.join("install/libexec/git-core");
    let actual_exec_path = capture(git_executable, ["--exec-path"], None, &[])?;
    if Path::new(actual_exec_path.trim()) != expected_exec_path.as_path() {
        return Ok(None);
    }
    Ok(Some(marker))
}

fn require_static_runtime(
    runtime_libraries: &BTreeMap<String, String>,
) -> Result<(), GitNativeBuildError> {
    if has_static_runtime(runtime_libraries) {
        return Ok(());
    }
    let dynamic = runtime_libraries
        .iter()
        .filter(|(_, identity)| identity.as_str() != "no dynamic library dependencies")
        .map(|(image, identity)| format!("{image}: {identity}"))
        .collect::<Vec<_>>();
    Err(GitNativeBuildError::RuntimeDependencyProbe(format!(
        "static musl candidate still has dynamic runtime dependencies: {}",
        dynamic.join(", ")
    )))
}

fn has_static_runtime(runtime_libraries: &BTreeMap<String, String>) -> bool {
    !runtime_libraries.is_empty()
        && runtime_libraries
            .values()
            .all(|identity| identity == "no dynamic library dependencies")
}

/// Record shared objects resolved by every executable ELF image in the Git
/// installation. Static candidates record one empty-closure entry per image;
/// dynamically linked candidates retain canonical paths and content digests.
/// Either form invalidates a cache hit if its runtime closure changes.
fn runtime_library_inventory(root: &Path) -> Result<BTreeMap<String, String>, GitNativeBuildError> {
    let mut executables = Vec::new();
    collect_elf_executables(root, root, &mut executables)?;
    executables.sort();
    let mut inventory = BTreeMap::new();
    for executable in executables {
        let relative = executable
            .strip_prefix(root)
            .map_err(|_| GitNativeBuildError::InventoryEscapesRoot(executable.clone()))?
            .to_string_lossy()
            .replace('\\', "/");
        let output = run_output("inspect native Git runtime dependencies", "ldd", [&executable], None, &[])?;
        if !output.status.success() {
            let details = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            if details.contains("not a dynamic executable") || details.contains("statically linked") {
                inventory.insert(format!("{relative}:static"), "no dynamic library dependencies".to_owned());
                continue;
            }
            return Err(command_failed("inspect native Git runtime dependencies", output));
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("linux-vdso") {
                continue;
            }
            let (name, resolved) = if let Some((name, target)) = line.split_once("=>") {
                let target = target.trim();
                if target == "not found" {
                    return Err(GitNativeBuildError::RuntimeDependencyProbe(format!(
                        "{} has an unresolved shared library: {}",
                        relative,
                        name.trim(),
                    )));
                }
                (name.trim().to_owned(), target.split_whitespace().next().unwrap_or_default())
            } else {
                let path = line.split_whitespace().next().unwrap_or_default();
                if !path.starts_with('/') {
                    return Err(GitNativeBuildError::RuntimeDependencyProbe(format!(
                        "unrecognized ldd output for {relative}: {line}"
                    )));
                }
                (Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned(), path)
            };
            let resolved_path = Path::new(resolved);
            if !resolved_path.is_absolute() || !resolved_path.is_file() {
                return Err(GitNativeBuildError::RuntimeDependencyProbe(format!(
                    "ldd reported an invalid path for {relative} {}: {resolved:?}", name
                )));
            }
            let canonical = fs::canonicalize(resolved_path)
                .map_err(|source| io_error("resolve native Git runtime library", resolved_path, source))?;
            let digest = sha256_file(&canonical)
                .map_err(|source| io_error("hash native Git runtime library", &canonical, source))?;
            inventory.insert(
                format!("{relative}:{name}"),
                format!("{} sha256:{digest}", canonical.display()),
            );
        }
    }
    Ok(inventory)
}

fn collect_elf_executables(
    root: &Path,
    directory: &Path,
    executables: &mut Vec<PathBuf>,
) -> Result<(), GitNativeBuildError> {
    let children = fs::read_dir(directory)
        .map_err(|source| io_error("scan Git runtime executables", directory, source))?;
    for child in children {
        let child = child.map_err(|source| io_error("scan Git runtime executable", directory, source))?;
        let path = child.path();
        let file_type = child
            .file_type()
            .map_err(|source| io_error("inspect Git runtime executable", &path, source))?;
        if file_type.is_dir() {
            collect_elf_executables(root, &path, executables)?;
        } else if file_type.is_file() && is_executable(&path) {
            let mut file = fs::File::open(&path)
                .map_err(|source| io_error("read Git runtime executable", &path, source))?;
            let mut magic = [0_u8; 4];
            let bytes_read = file
                .read(&mut magic)
                .map_err(|source| io_error("read Git runtime executable header", &path, source))?;
            if bytes_read == magic.len() && magic == *b"\x7fELF" {
                if !path.starts_with(root) {
                    return Err(GitNativeBuildError::InventoryEscapesRoot(path));
                }
                executables.push(path);
            }
        }
    }
    Ok(())
}

fn acquire_source(path: &Path, url: &str, expected_sha256: &str) -> Result<(), GitNativeBuildError> {
    if path.is_file() {
        match sha256_file(path) {
            Ok(actual) if actual == expected_sha256 => return Ok(()),
            Ok(_) | Err(_) => remove_path(path)?,
        }
    }
    let parent = path.parent().ok_or_else(|| {
        GitNativeBuildError::Metadata(format!("source path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|source| io_error("create source archive directory", parent, source))?;
    let temporary = parent.join(format!(".{}.{}.download", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let result = (|| {
        let mut response = reqwest::blocking::Client::builder()
            .user_agent("Vapor managed Git source builder")
            .build()
            .map_err(|error| GitNativeBuildError::Download(error.to_string()))?
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| GitNativeBuildError::Download(error.to_string()))?;
        if response.content_length().is_some_and(|size| size > MAX_SOURCE_ARCHIVE_BYTES) {
            return Err(GitNativeBuildError::SourceArchiveTooLarge(url.to_owned()));
        }
        let mut file = fs::File::create(&temporary)
            .map_err(|source| io_error("create temporary source archive", &temporary, source))?;
        let bytes = io::copy(&mut response.by_ref().take(MAX_SOURCE_ARCHIVE_BYTES + 1), &mut file)
            .map_err(|error| GitNativeBuildError::Download(error.to_string()))?;
        if bytes > MAX_SOURCE_ARCHIVE_BYTES {
            return Err(GitNativeBuildError::SourceArchiveTooLarge(url.to_owned()));
        }
        file.sync_all().map_err(|source| io_error("sync source archive", &temporary, source))?;
        let actual_sha256 = sha256_file(&temporary)
            .map_err(|error| GitNativeBuildError::Download(error.to_string()))?;
        if actual_sha256 != expected_sha256 {
            return Err(GitNativeBuildError::SourceDigestMismatch {
                source: url.to_owned(),
                expected: expected_sha256.to_owned(),
                actual: actual_sha256,
            });
        }
        fs::rename(&temporary, path).map_err(|source| io_error("promote verified source archive", path, source))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn extract_source(
    archive: &Path,
    destination: &Path,
    expected_top_level: &str,
    xz: bool,
) -> Result<(), GitNativeBuildError> {
    let sentinel = if expected_top_level.starts_with("make-") {
        "build.sh"
    } else if expected_top_level.starts_with("openssl-") {
        "Configure"
    } else {
        "configure"
    };
    if destination.join(sentinel).is_file() {
        return Ok(());
    }
    remove_path(destination)?;
    let parent = destination.parent().ok_or_else(|| {
        GitNativeBuildError::Metadata(format!("source destination has no parent: {}", destination.display()))
    })?;
    fs::create_dir_all(parent).map_err(|source| io_error("create source extraction parent", parent, source))?;
    validate_archive_paths(archive, expected_top_level, xz)?;
    fs::create_dir_all(destination).map_err(|source| io_error("create source extraction directory", destination, source))?;
    let archive_arg = if xz { "-xJf" } else { "-xzf" };
    let output = run_output(
        "extract verified upstream source",
        "tar",
        [archive_arg, &archive.to_string_lossy(), "-C", &destination.to_string_lossy(), "--strip-components=1", "--no-same-owner"],
        None,
        &[],
    )?;
    if !output.status.success() {
        let _ = remove_path(destination);
        return Err(command_failed("extract verified upstream source", output));
    }
    if !destination.join(sentinel).is_file() {
        let _ = remove_path(destination);
        return Err(GitNativeBuildError::IncompleteSourceTree {
            archive: archive.to_path_buf(),
            expected: destination.join(sentinel),
        });
    }
    Ok(())
}

fn validate_archive_paths(
    archive: &Path,
    expected_top_level: &str,
    xz: bool,
) -> Result<(), GitNativeBuildError> {
    let list_arg = if xz { "-tJf" } else { "-tzf" };
    let output = run_output(
        "inspect upstream source archive",
        "tar",
        [list_arg, &archive.to_string_lossy()],
        None,
        &[],
    )?;
    if !output.status.success() {
        return Err(command_failed("inspect upstream source archive", output));
    }
    for entry in String::from_utf8_lossy(&output.stdout).lines() {
        let path = Path::new(entry);
        let mut components = path.components();
        let first = components.next();
        if path.is_absolute()
            || !matches!(first, Some(Component::Normal(name)) if name == expected_top_level)
            || components.any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            return Err(GitNativeBuildError::UnsafeArchivePath(entry.to_owned()));
        }
    }
    Ok(())
}

fn prepend_path(directory: &Path) -> Result<String, GitNativeBuildError> {
    let mut paths = vec![directory.to_path_buf()];
    if let Some(path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&path));
    }
    env::join_paths(paths)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| GitNativeBuildError::Metadata(format!("could not construct compiler PATH: {error}")))
}

fn program_reports_version(path: &Path, arg: &str, expected: &str) -> Result<bool, GitNativeBuildError> {
    if !path.is_file() {
        return Ok(false);
    }
    let output = Command::new(path).arg(arg).output().map_err(|source| {
        io_error("inspect cached native build tool", path, source)
    })?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(output.status.success() && text.lines().any(|line| line.contains(expected)))
}

fn read_git_version(path: &Path) -> Result<String, GitNativeBuildError> {
    capture(path, ["--version"], None, &[]).map(|version| version.trim().to_owned())
}

fn capture<I, S>(
    program: impl AsRef<std::ffi::OsStr>,
    args: I,
    cwd: Option<&Path>,
    environment: &[(&str, &str)],
) -> Result<String, GitNativeBuildError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = run_output("run managed build probe", program, args, cwd, environment)?;
    if !output.status.success() {
        return Err(command_failed("run managed build probe", output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_output<I, S>(
    operation: &'static str,
    program: impl AsRef<std::ffi::OsStr>,
    args: I,
    cwd: Option<&Path>,
    environment: &[(&str, &str)],
) -> Result<Output, GitNativeBuildError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in environment {
        command.env(key, value);
    }
    command.output().map_err(|source| GitNativeBuildError::ProcessStart {
        operation,
        source,
    })
}

fn command_failed(operation: &'static str, output: Output) -> GitNativeBuildError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let details = if stderr.is_empty() { stdout } else { stderr };
    GitNativeBuildError::ProcessFailed {
        operation,
        status: output.status.to_string(),
        details: truncate(&details, 4000),
    }
}

fn inventory_digest(root: &Path) -> Result<String, GitNativeBuildError> {
    let root = fs::canonicalize(root).map_err(|source| io_error("resolve Git install inventory", root, source))?;
    let mut entries = Vec::new();
    collect_inventory(&root, &root, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut bytes = Vec::new();
    for (path, identity) in entries {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(identity.as_bytes());
        bytes.push(b'\n');
    }
    Ok(sha256_bytes(&bytes))
}

fn collect_inventory(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<(String, String)>,
) -> Result<(), GitNativeBuildError> {
    let children = fs::read_dir(directory).map_err(|source| io_error("read Git install inventory", directory, source))?;
    for child in children {
        let child = child.map_err(|source| io_error("read Git install inventory entry", directory, source))?;
        let path = child.path();
        let relative = path.strip_prefix(root).map_err(|_| GitNativeBuildError::InventoryEscapesRoot(path.clone()))?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        let kind = child.file_type().map_err(|source| io_error("inspect Git install inventory entry", &path, source))?;
        if kind.is_dir() {
            collect_inventory(root, &path, entries)?;
        } else if kind.is_symlink() {
            let target = fs::read_link(&path).map_err(|source| io_error("read Git install symlink", &path, source))?;
            if target.is_absolute() {
                return Err(GitNativeBuildError::InvalidInstallSymlink(path));
            }
            let resolved = fs::canonicalize(&path).map_err(|source| io_error("resolve Git install symlink", &path, source))?;
            if !resolved.starts_with(root) {
                return Err(GitNativeBuildError::InvalidInstallSymlink(path));
            }
            entries.push((relative, format!("symlink:{}", target.to_string_lossy())));
        } else if kind.is_file() {
            let contents = fs::read(&path).map_err(|source| io_error("read Git install file", &path, source))?;
            #[cfg(unix)]
            let executable_bits = {
                use std::os::unix::fs::PermissionsExt;
                fs::metadata(&path)
                    .map_err(|source| io_error("read Git install permissions", &path, source))?
                    .permissions()
                    .mode()
                    & 0o111
            };
            #[cfg(not(unix))]
            let executable_bits = 0;
            entries.push((relative, format!("file:{executable_bits:o}:{}", sha256_bytes(&contents))));
        } else {
            return Err(GitNativeBuildError::UnsupportedInstallEntry(path));
        }
    }
    Ok(())
}

fn write_marker(entry_root: &Path, marker: &BuildMarker) -> Result<(), GitNativeBuildError> {
    let marker_path = entry_root.join(MARKER_NAME);
    let temporary = entry_root.join(format!(".{MARKER_NAME}.{}.tmp", std::process::id()));
    let text = toml::to_string_pretty(marker)
        .map_err(|error| GitNativeBuildError::Metadata(format!("could not encode build marker: {error}")))?;
    let mut file = fs::File::create(&temporary)
        .map_err(|source| io_error("write Git build marker", &temporary, source))?;
    file.write_all(text.as_bytes()).map_err(|source| io_error("write Git build marker", &temporary, source))?;
    file.sync_all().map_err(|source| io_error("sync Git build marker", &temporary, source))?;
    fs::rename(&temporary, &marker_path)
        .map_err(|source| io_error("promote Git build marker", &marker_path, source))
}

fn remove_path(path: &Path) -> Result<(), GitNativeBuildError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|source| io_error("remove owned Git build output", path, source))
    } else {
        fs::remove_file(path).map_err(|source| io_error("remove owned Git build output", path, source))
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&SHA256);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> GitNativeBuildError {
    GitNativeBuildError::Io {
        operation,
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &value[start..])
}

struct BuildLock(PathBuf);

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

#[derive(Debug)]
pub enum GitNativeBuildError {
    UnsupportedHost,
    ManagedCompilerUnavailable(PathBuf),
    BuildAlreadyRunning(PathBuf),
    Download(String),
    SourceDigestMismatch { source: String, expected: String, actual: String },
    SourceArchiveTooLarge(String),
    IncompleteSourceTree { archive: PathBuf, expected: PathBuf },
    UnsafeArchivePath(String),
    RuntimeDependencyProbe(String),
    MissingStaticDependency(PathBuf),
    UnexpectedToolVersion { tool: &'static str, expected: String, path: PathBuf },
    UnexpectedGitVersion { expected: String, actual: String },
    UnexpectedGitExecPath { expected: PathBuf, actual: String },
    IncompleteGitInstall(PathBuf),
    InventoryEscapesRoot(PathBuf),
    InvalidInstallSymlink(PathBuf),
    UnsupportedInstallEntry(PathBuf),
    ProcessStart { operation: &'static str, source: io::Error },
    ProcessFailed { operation: &'static str, status: String, details: String },
    Io { operation: &'static str, path: PathBuf, message: String },
    Metadata(String),
}

impl std::fmt::Display for GitNativeBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedHost => formatter.write_str("the Vapor-managed source Git shadow build currently supports x86-64 Linux only"),
            Self::ManagedCompilerUnavailable(path) => write!(formatter, "Vapor-managed Zig is unavailable or incomplete at '{}'", path.display()),
            Self::BuildAlreadyRunning(path) => write!(formatter, "a Git native build is already active in '{}'", path.display()),
            Self::Download(message) => write!(formatter, "could not acquire pinned Git build source: {message}"),
            Self::SourceDigestMismatch { source, expected, actual } => write!(formatter, "pinned source digest mismatch for {source}: expected {expected}, got {actual}"),
            Self::SourceArchiveTooLarge(url) => write!(formatter, "pinned source archive exceeds the size limit: {url}"),
            Self::IncompleteSourceTree { archive, expected } => write!(formatter, "source archive '{}' did not produce required file '{}'", archive.display(), expected.display()),
            Self::UnsafeArchivePath(path) => write!(formatter, "upstream source archive contains an unsafe path: {path}"),
            Self::RuntimeDependencyProbe(message) => write!(formatter, "could not verify the pinned static Git dependencies: {message}"),
            Self::MissingStaticDependency(path) => write!(formatter, "static Git dependency build did not produce '{}'", path.display()),
            Self::UnexpectedToolVersion { tool, expected, path } => write!(formatter, "{} at '{}' did not report the expected version {expected}", tool, path.display()),
            Self::UnexpectedGitVersion { expected, actual } => write!(formatter, "source Git reported '{actual}', expected git version {expected}"),
            Self::UnexpectedGitExecPath { expected, actual } => write!(formatter, "source Git helper path was '{actual}', expected '{}'", expected.display()),
            Self::IncompleteGitInstall(path) => write!(formatter, "Git shadow build is missing git or its HTTPS remote helper under '{}'", path.display()),
            Self::InventoryEscapesRoot(path) => write!(formatter, "Git build inventory entry escapes its install root: '{}'", path.display()),
            Self::InvalidInstallSymlink(path) => write!(formatter, "Git install contains an absolute, broken, or escaping symlink: '{}'", path.display()),
            Self::UnsupportedInstallEntry(path) => write!(formatter, "Git install contains an unsupported filesystem entry: '{}'", path.display()),
            Self::ProcessStart { operation, source } => write!(formatter, "could not {operation}: {source}"),
            Self::ProcessFailed { operation, status, details } => write!(formatter, "could not {operation} ({status}): {details}"),
            Self::Io { operation, path, message } => write!(formatter, "could not {operation} at '{}': {message}", path.display()),
            Self::Metadata(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for GitNativeBuildError {}
