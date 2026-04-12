use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;

/// Information about the connected peer process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerInfo {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

/// Access policy for the Limux Unix control socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketControlMode {
    /// Allow only descendant processes of the Limux server from the same user.
    LimuxOnly,
    /// Allow any connection from the same local user.
    LocalUser,
    /// Allow any local connection that can reach the socket path.
    AllowAll,
}

impl SocketControlMode {
    pub fn from_env() -> Self {
        std::env::var("LIMUX_SOCKET_MODE")
            .ok()
            .or_else(|| std::env::var("CMUX_SOCKET_MODE").ok())
            .as_deref()
            .map(Self::parse)
            .unwrap_or(Self::LocalUser)
    }

    fn parse(value: &str) -> Self {
        match value.trim() {
            "allowAll" | "allow-all" | "allow_all" => Self::AllowAll,
            "localUser" | "local-user" | "local_user" => Self::LocalUser,
            "cmuxOnly" | "limuxOnly" | "descendantOnly" | "descendant-only" | "descendant_only" => {
                Self::LimuxOnly
            }
            _ => Self::LocalUser,
        }
    }

    pub fn requires_owner_only_socket(self) -> bool {
        matches!(self, Self::LimuxOnly | Self::LocalUser)
    }
}

pub fn authorize_peer<S: AsRawFd>(stream: &S, mode: SocketControlMode) -> io::Result<PeerInfo> {
    let peer = peer_info(stream)?;
    if is_authorized(&peer, mode) {
        Ok(peer)
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("unauthorized peer uid={} pid={}", peer.uid, peer.pid),
        ))
    }
}

pub fn is_authorized(peer: &PeerInfo, mode: SocketControlMode) -> bool {
    match mode {
        SocketControlMode::AllowAll => true,
        SocketControlMode::LimuxOnly => peer.uid == current_uid() && is_descendant(peer.pid),
        SocketControlMode::LocalUser => peer.uid == current_uid(),
    }
}

fn peer_info<S: AsRawFd>(stream: &S) -> io::Result<PeerInfo> {
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut cred_len = size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut cred_len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if cred_len != size_of::<libc::ucred>() as libc::socklen_t {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected peer credential size",
        ));
    }

    Ok(PeerInfo {
        pid: u32::try_from(cred.pid).unwrap_or(0),
        uid: cred.uid,
        gid: cred.gid,
    })
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn is_descendant(pid: u32) -> bool {
    let ancestor_pid = std::process::id();
    if pid == 0 {
        return false;
    }

    let mut current = pid;
    for _ in 0..64 {
        if current == ancestor_pid {
            return true;
        }
        if current <= 1 {
            return false;
        }
        match read_ppid(current) {
            Some(parent) if parent != current => current = parent,
            _ => return false,
        }
    }

    false
}

fn read_ppid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("PPid:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Mutex;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let old = std::env::var_os(key);
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn socket_mode_defaults_to_local_user() {
        let _lock = ENV_TEST_LOCK.lock().expect("env lock");
        let _limux = EnvGuard::set("LIMUX_SOCKET_MODE", None);
        let _cmux = EnvGuard::set("CMUX_SOCKET_MODE", None);
        assert_eq!(SocketControlMode::from_env(), SocketControlMode::LocalUser);
    }

    #[test]
    fn descendant_aliases_map_to_limux_only() {
        let _lock = ENV_TEST_LOCK.lock().expect("env lock");
        let _limux = EnvGuard::set("LIMUX_SOCKET_MODE", Some("cmuxOnly"));
        let _cmux = EnvGuard::set("CMUX_SOCKET_MODE", None);
        assert_eq!(SocketControlMode::from_env(), SocketControlMode::LimuxOnly);
    }

    #[test]
    fn allow_all_accepts_any_uid() {
        let peer = PeerInfo {
            pid: 42,
            uid: current_uid().saturating_add(1),
            gid: 7,
        };
        assert!(is_authorized(&peer, SocketControlMode::AllowAll));
    }

    #[test]
    fn limux_only_allows_current_process() {
        let peer = PeerInfo {
            pid: std::process::id(),
            uid: current_uid(),
            gid: unsafe { libc::getgid() },
        };
        assert!(is_authorized(&peer, SocketControlMode::LimuxOnly));
    }

    #[test]
    fn limux_only_rejects_non_descendant_pid() {
        let peer = PeerInfo {
            pid: 1,
            uid: current_uid(),
            gid: unsafe { libc::getgid() },
        };
        assert!(!is_authorized(&peer, SocketControlMode::LimuxOnly));
    }

    #[test]
    fn authorize_peer_reads_same_user_credentials() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let socket_path = temp_dir.path().join("auth.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind listener");
        let client = UnixStream::connect(&socket_path).expect("connect client");
        let (server, _) = listener.accept().expect("accept client");

        let peer = authorize_peer(&server, SocketControlMode::LocalUser).expect("authorize");

        assert_eq!(peer.uid, current_uid());
        assert_eq!(peer.gid, unsafe { libc::getgid() });

        drop(client);
    }
}
