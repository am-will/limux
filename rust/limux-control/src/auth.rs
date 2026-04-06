use std::io;

/// Information about the connected peer process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerInfo {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

/// Socket control mode for the Limux Unix control socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketControlMode {
    /// Allow only descendant processes of the Limux server from the same user.
    LimuxOnly,
    /// Allow any connection from the same local user.
    LocalUser,
    /// Allow any local connection.
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
}

pub fn authenticate_peer(stream: &tokio::net::UnixStream) -> io::Result<PeerInfo> {
    let cred = stream.peer_cred()?;
    Ok(PeerInfo {
        pid: cred
            .pid()
            .and_then(|pid| u32::try_from(pid).ok())
            .unwrap_or(0),
        uid: cred.uid(),
        gid: cred.gid(),
    })
}

pub fn is_authorized(peer: &PeerInfo, mode: SocketControlMode, server_pid: u32) -> bool {
    match mode {
        SocketControlMode::AllowAll => true,
        SocketControlMode::LocalUser => is_same_user(peer),
        SocketControlMode::LimuxOnly => is_same_user(peer) && is_descendant(peer.pid, server_pid),
    }
}

fn is_same_user(peer: &PeerInfo) -> bool {
    peer.uid == unsafe { libc::getuid() }
}

fn is_descendant(pid: u32, ancestor_pid: u32) -> bool {
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
    fn socket_mode_accepts_cmux_compat_values() {
        let _lock = ENV_TEST_LOCK.lock().expect("env lock");
        let _limux = EnvGuard::set("LIMUX_SOCKET_MODE", Some("cmuxOnly"));
        let _cmux = EnvGuard::set("CMUX_SOCKET_MODE", None);
        assert_eq!(SocketControlMode::from_env(), SocketControlMode::LimuxOnly);
    }

    #[test]
    fn limux_only_allows_current_process() {
        let peer = PeerInfo {
            pid: std::process::id(),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        };
        assert!(is_authorized(
            &peer,
            SocketControlMode::LimuxOnly,
            std::process::id()
        ));
    }

    #[test]
    fn limux_only_rejects_non_descendant_pid() {
        let peer = PeerInfo {
            pid: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        };
        assert!(!is_authorized(
            &peer,
            SocketControlMode::LimuxOnly,
            std::process::id()
        ));
    }
}
