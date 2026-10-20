//! Cross-platform local IPC used by all Vapor Client frontends.

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, ToNsName, traits::Stream as _,
};
pub use interprocess::local_socket::{Listener, Stream};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub fn endpoint_name(app_root: Option<&Path>, state_directory: &Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut identity = Vec::new();
    if let Some(app_root) = app_root {
        identity.extend_from_slice(app_root.to_string_lossy().as_bytes());
    }
    identity.push(0xff);
    identity.extend_from_slice(state_directory.to_string_lossy().as_bytes());
    for byte in identity {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("vapor-client-{hash:016x}")
}

pub fn connect(endpoint: &str) -> io::Result<Stream> {
    let name = endpoint
        .to_ns_name::<GenericNamespaced>()
        .map_err(io::Error::other)?;
    Stream::connect(name)
}

pub fn bind(endpoint: &str) -> io::Result<Listener> {
    let name = endpoint
        .to_ns_name::<GenericNamespaced>()
        .map_err(io::Error::other)?;
    bind_with_local_permissions(name)
}

#[cfg(unix)]
fn bind_with_local_permissions(name: interprocess::local_socket::Name<'_>) -> io::Result<Listener> {
    use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
    ListenerOptions::new().name(name).mode(0o600).create_sync()
}

#[cfg(windows)]
fn bind_with_local_permissions(name: interprocess::local_socket::Name<'_>) -> io::Result<Listener> {
    use interprocess::os::windows::{
        local_socket::ListenerOptionsExt as _, security_descriptor::SecurityDescriptor,
    };
    use widestring::U16CString;

    let sddl = U16CString::from_str("D:P(A;;GA;;;SY)(A;;GA;;;OW)").map_err(io::Error::other)?;
    let security = SecurityDescriptor::deserialize(&sddl)?;
    ListenerOptions::new()
        .name(name)
        .security_descriptor(security)
        .create_sync()
}

#[cfg(not(any(unix, windows)))]
fn bind_with_local_permissions(name: interprocess::local_socket::Name<'_>) -> io::Result<Listener> {
    ListenerOptions::new().name(name).create_sync()
}

/// Serializes frontends that are starting the service, through readiness.
pub fn bootstrap_lock(state_directory: &Path) -> io::Result<File> {
    lock_file(state_directory, "service-bootstrap.lock")
}

/// Serializes service processes while they inspect and claim the IPC endpoint.
pub fn service_lock(state_directory: &Path) -> io::Result<File> {
    lock_file(state_directory, "service-instance.lock")
}

fn lock_file(state_directory: &Path, filename: &str) -> io::Result<File> {
    fs::create_dir_all(state_directory)?;
    let lock_path = state_directory.join(filename);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock()?;
    Ok(lock)
}

/// Remove a stale filesystem socket name without displacing a live listener.
pub fn remove_stale_endpoint(endpoint: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        // GenericNamespaced uses abstract sockets on Linux and `/tmp/<name>` on
        // other Unix targets. The Linux abstract namespace cannot leave a stale
        // filesystem entry.
        if !cfg!(target_os = "linux") {
            let path = PathBuf::from("/tmp").join(endpoint);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path)?,
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "local service endpoint exists but is not a socket",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    #[cfg(not(unix))]
    let _ = endpoint;
    Ok(())
}

pub fn app_root(executable: &Path) -> Option<PathBuf> {
    executable
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::{bootstrap_lock, service_lock};
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, SystemTime},
    };

    #[test]
    fn service_can_claim_its_endpoint_while_frontend_holds_bootstrap_lock() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-client-startup-locks-{}-{unique}",
            std::process::id()
        ));
        let bootstrap = bootstrap_lock(&root).expect("lock service startup");
        let service_root = root.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = service_lock(&service_root).map(|_| ());
            let _ = sender.send(result);
        });

        let acquired_without_waiting = receiver
            .recv_timeout(Duration::from_millis(250))
            .is_ok_and(|result| result.is_ok());
        drop(bootstrap);
        worker.join().expect("service lock worker exits");
        let _ = std::fs::remove_dir_all(root);

        assert!(
            acquired_without_waiting,
            "service endpoint lock must not contend with frontend startup lock"
        );
    }
}
