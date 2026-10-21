//! Cross-platform local IPC used by all Vapor Client frontends.

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, ToNsName, traits::Stream as _,
};
pub use interprocess::local_socket::{Listener, Stream};
#[cfg(unix)]
use std::fs::{self, File, OpenOptions};
use std::{
    io,
    path::{Path, PathBuf},
};

/// An interprocess startup guard. Windows uses a named kernel mutex because
/// Windows file-range locks are not implemented by every supported runtime
/// (including Wine); Unix keeps the existing advisory file lock.
pub struct StartupLock {
    _guard: StartupLockGuard,
}

#[cfg(unix)]
struct StartupLockGuard {
    _file: File,
}

#[cfg(windows)]
struct StartupLockGuard {
    _handle: windows_sys::Win32::Foundation::HANDLE,
}

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

    // OWNER RIGHTS (S-1-3-4) controls owner permissions; it does not grant
    // access to the process that happens to own the pipe. Name the interactive
    // account explicitly so its frontends can connect without opening the pipe
    // to every authenticated user on a shared machine.
    let user_sid = current_user_sid()?;
    let sddl = U16CString::from_str(format!("D:P(A;;GA;;;SY)(A;;GA;;;{user_sid})"))
        .map_err(io::Error::other)?;
    let security = SecurityDescriptor::deserialize(&sddl)?;
    ListenerOptions::new()
        .name(name)
        .security_descriptor(security)
        .create_sync()
}

#[cfg(windows)]
fn current_user_sid() -> io::Result<String> {
    use std::{
        mem::{offset_of, size_of},
        ptr::{self, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        let mut required = 0_u32;
        let first_query =
            unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut required) };
        if first_query != 0 || required < size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }

        // Keep TOKEN_USER naturally aligned; it contains a pointer to the SID.
        let words = (required as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                storage.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        let token_user = storage.as_ptr().cast::<TOKEN_USER>();
        let sid = unsafe { (*token_user).User.Sid };
        if sid.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "current token has no user SID",
            ));
        }
        let header =
            unsafe { ptr::read_unaligned(sid.cast::<windows_sys::Win32::Security::SID>()) };
        let count = usize::from(header.SubAuthorityCount);
        if count > 15 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "current token has an invalid user SID",
            ));
        }
        let authority = header
            .IdentifierAuthority
            .Value
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
        let subauthority = unsafe {
            (0..count)
                .map(|index| {
                    ptr::read_unaligned(
                        sid.cast::<u8>()
                            .add(
                                offset_of!(windows_sys::Win32::Security::SID, SubAuthority)
                                    + index * size_of::<u32>(),
                            )
                            .cast::<u32>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut text = format!("S-{}-{authority}", header.Revision);
        for part in subauthority {
            text.push('-');
            text.push_str(&part.to_string());
        }
        Ok(text)
    })();

    unsafe { CloseHandle(token) };
    result
}

#[cfg(not(any(unix, windows)))]
fn bind_with_local_permissions(name: interprocess::local_socket::Name<'_>) -> io::Result<Listener> {
    ListenerOptions::new().name(name).create_sync()
}

/// Serializes frontends that are starting the service, through readiness.
pub fn bootstrap_lock(state_directory: &Path) -> io::Result<StartupLock> {
    lock_file(state_directory, "service-bootstrap.lock")
}

/// Serializes service processes while they inspect and claim the IPC endpoint.
pub fn service_lock(state_directory: &Path) -> io::Result<StartupLock> {
    lock_file(state_directory, "service-instance.lock")
}

fn lock_file(state_directory: &Path, filename: &str) -> io::Result<StartupLock> {
    #[cfg(unix)]
    {
        fs::create_dir_all(state_directory)?;
        let lock_path = state_directory.join(filename);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.lock()?;
        Ok(StartupLock {
            _guard: StartupLockGuard { _file: lock },
        })
    }

    #[cfg(windows)]
    {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in state_directory
            .to_string_lossy()
            .to_lowercase()
            .bytes()
            .chain([0xff])
            .chain(filename.bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let name: Vec<u16> = format!("Local\\VaporClientStartup-{hash:016x}")
            .encode_utf16()
            .chain([0])
            .collect();
        let handle = unsafe {
            windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr())
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let wait = unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                handle,
                windows_sys::Win32::System::Threading::INFINITE,
            )
        };
        if wait != windows_sys::Win32::Foundation::WAIT_OBJECT_0
            && wait != windows_sys::Win32::Foundation::WAIT_ABANDONED
        {
            let error = if wait == windows_sys::Win32::Foundation::WAIT_FAILED {
                io::Error::last_os_error()
            } else {
                io::Error::other(format!("unexpected startup mutex wait result {wait}"))
            };
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return Err(error);
        }
        Ok(StartupLock {
            _guard: StartupLockGuard { _handle: handle },
        })
    }
}

#[cfg(windows)]
impl Drop for StartupLockGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self._handle);
            windows_sys::Win32::Foundation::CloseHandle(self._handle);
        }
    }
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
