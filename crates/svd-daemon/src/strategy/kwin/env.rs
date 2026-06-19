use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::strategy::StrategyError;

/// Candidate Wayland session derived from a trusted runtime directory and socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KWinEnv {
    pub uid: u32,
    pub gid: u32,
    pub wayland_display: String,
    pub xdg_runtime_dir: String,
}

pub fn discover_for_uid(requester_uid: Option<u32>) -> Result<Vec<KWinEnv>, StrategyError> {
    discover_in(Path::new("/run/user"), requester_uid)
}

fn discover_in(
    runtime_root: &Path,
    requester_uid: Option<u32>,
) -> Result<Vec<KWinEnv>, StrategyError> {
    let effective_uid = unsafe { libc::geteuid() };
    let scope_uid = requester_uid.unwrap_or(effective_uid);
    let runtime_dirs: Vec<PathBuf> = if scope_uid == 0 {
        fs::read_dir(runtime_root)
            .map_err(StrategyError::Io)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                    .map(|_| entry.path())
            })
            .collect()
    } else {
        vec![runtime_root.join(scope_uid.to_string())]
    };

    let mut candidates = Vec::new();
    for runtime_dir in runtime_dirs {
        let Some(uid) = runtime_dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let metadata = match fs::symlink_metadata(&runtime_dir) {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::debug!(path = %runtime_dir.display(), %error, "discarding runtime directory");
                continue;
            }
        };
        if !metadata.file_type().is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid
            || metadata.mode() & 0o7777 != 0o700
        {
            tracing::debug!(
                path = %runtime_dir.display(),
                expected_uid = uid,
                actual_uid = metadata.uid(),
                mode = format_args!("{:04o}", metadata.mode() & 0o7777),
                "discarding untrusted runtime directory"
            );
            continue;
        }

        let entries = match fs::read_dir(&runtime_dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::debug!(path = %runtime_dir.display(), %error, "discarding unreadable runtime directory");
                continue;
            }
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(suffix) = name.strip_prefix("wayland-") else {
                continue;
            };
            if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let socket_metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) => {
                    tracing::debug!(path = %entry.path().display(), %error, "discarding Wayland candidate");
                    continue;
                }
            };
            if !socket_metadata.file_type().is_socket() || socket_metadata.uid() != uid {
                tracing::debug!(
                    path = %entry.path().display(),
                    owner = socket_metadata.uid(),
                    "discarding untrusted Wayland candidate"
                );
                continue;
            }
            let candidate = KWinEnv {
                uid,
                gid: metadata.gid(),
                wayland_display: name.to_owned(),
                xdg_runtime_dir: runtime_dir.to_string_lossy().into_owned(),
            };
            tracing::debug!(
                uid,
                gid = candidate.gid,
                wayland_display = %candidate.wayland_display,
                "discovered Wayland candidate"
            );
            candidates.push(candidate);
        }
    }
    candidates.sort_by(|left, right| {
        (left.uid, &left.wayland_display).cmp(&(right.uid, &right.wayland_display))
    });
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("svd-env-test-{}-{sequence}", std::process::id()));
            fs::create_dir(&path).expect("create temporary root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn current_ids() -> (u32, u32) {
        let metadata = fs::metadata("/proc/self").expect("process metadata");
        (metadata.uid(), metadata.gid())
    }

    fn runtime_dir(root: &Path, uid: u32) -> PathBuf {
        let path = root.join(uid.to_string());
        fs::create_dir(&path).expect("create runtime directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("set runtime permissions");
        path
    }

    #[test]
    fn discovery_accepts_owned_wayland_socket() {
        let root = TempDir::new();
        let (uid, gid) = current_ids();
        let runtime = runtime_dir(root.path(), uid);
        let _socket = UnixListener::bind(runtime.join("wayland-7")).expect("bind socket");

        let candidates = discover_in(root.path(), Some(uid)).expect("discover candidates");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].uid, uid);
        assert_eq!(candidates[0].gid, gid);
        assert_eq!(candidates[0].wayland_display, "wayland-7");
        assert_eq!(candidates[0].xdg_runtime_dir, runtime.to_string_lossy());
    }

    #[test]
    fn discovery_ignores_regular_files_and_lock_files() {
        let root = TempDir::new();
        let (uid, _) = current_ids();
        let runtime = runtime_dir(root.path(), uid);
        fs::write(runtime.join("wayland-0"), b"not a socket").expect("write regular file");
        fs::write(runtime.join("wayland-1.lock"), b"").expect("write lock file");

        assert!(discover_in(root.path(), Some(uid))
            .expect("discover candidates")
            .is_empty());
    }

    #[test]
    fn discovery_rejects_symlink_socket() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new();
        let (uid, _) = current_ids();
        let runtime = runtime_dir(root.path(), uid);
        let _socket = UnixListener::bind(runtime.join("real-socket")).expect("bind socket");
        symlink("real-socket", runtime.join("wayland-0")).expect("create socket symlink");

        assert!(discover_in(root.path(), Some(uid))
            .expect("discover candidates")
            .is_empty());
    }

    #[test]
    fn discovery_rejects_symlink_runtime_directory() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new();
        let (uid, _) = current_ids();
        let target = root.path().join("runtime-target");
        fs::create_dir(&target).expect("create runtime target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
            .expect("set runtime permissions");
        let _socket = UnixListener::bind(target.join("wayland-0")).expect("bind socket");
        symlink(&target, root.path().join(uid.to_string())).expect("create runtime symlink");

        assert!(discover_in(root.path(), Some(uid))
            .expect("discover candidates")
            .is_empty());
    }

    #[test]
    fn discovery_rejects_insecure_runtime_directory_mode() {
        let root = TempDir::new();
        let (uid, _) = current_ids();
        let runtime = runtime_dir(root.path(), uid);
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o750))
            .expect("set insecure permissions");
        let _socket = UnixListener::bind(runtime.join("wayland-0")).expect("bind socket");

        assert!(discover_in(root.path(), Some(uid))
            .expect("discover candidates")
            .is_empty());
    }

    #[test]
    fn discovery_rejects_runtime_directory_owned_by_another_uid() {
        let root = TempDir::new();
        let (uid, _) = current_ids();
        let claimed_uid = uid.checked_add(1).expect("test uid increment");
        let runtime = runtime_dir(root.path(), claimed_uid);
        let _socket = UnixListener::bind(runtime.join("wayland-0")).expect("bind socket");

        assert!(discover_in(root.path(), Some(0))
            .expect("discover candidates")
            .is_empty());
    }
}
