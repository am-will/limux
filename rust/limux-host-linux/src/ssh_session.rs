//! Persisted SSH sessions contain structured data, never an arbitrary command.
use crate::ssh_hosts::SshTarget;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "SavedConnection", into = "SavedConnection")]
pub struct SshConnection {
    target: SshTarget,
    session: String,
    pub auto_reconnect: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedConnection {
    destination: String,
    port: Option<u16>,
    session: String,
    auto_reconnect: bool,
}

impl TryFrom<SavedConnection> for SshConnection {
    type Error = String;
    fn try_from(value: SavedConnection) -> Result<Self, String> {
        if !value.session.starts_with("limux-")
            || uuid::Uuid::parse_str(&value.session[6..])
                .map_or(true, |id| id.to_string() != value.session[6..])
        {
            return Err("Invalid Limux tmux session identity".into());
        }
        Ok(Self {
            target: SshTarget::parse(
                &value.destination,
                &value.port.map(|p| p.to_string()).unwrap_or_default(),
            )?,
            session: value.session,
            auto_reconnect: value.auto_reconnect,
        })
    }
}

impl From<SshConnection> for SavedConnection {
    fn from(value: SshConnection) -> Self {
        Self {
            destination: value.target.destination().to_string(),
            port: value.target.port(),
            session: value.session,
            auto_reconnect: value.auto_reconnect,
        }
    }
}

impl SshConnection {
    pub fn new(target: SshTarget, auto_reconnect: bool) -> Self {
        Self {
            target,
            session: format!("limux-{}", uuid::Uuid::new_v4()),
            auto_reconnect,
        }
    }

    pub fn new_tab(&self) -> Self {
        Self::new(self.target.clone(), self.auto_reconnect)
    }

    pub fn command(&self) -> String {
        // Both shells receive quoted arguments. Only our validated session ID
        // enters the remote command. No helper installation or tmux options.
        let remote = format!("exec tmux new-session -A -s '{}'", self.session);
        let mut args = vec![
            "env".to_string(),
            "TERM=xterm-256color".into(),
            "ssh".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
        ];
        args.extend(self.target.arguments());
        args.push(remote);
        args.iter()
            .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// OpenSSH reports connection/authentication errors as 255. Ghostty's POSIX
/// child watcher reports the exit status directly. Other remote exit statuses
/// must not trigger retries (e.g. missing tmux or an intentional detach).
pub fn should_reconnect(code: Option<u32>, enabled: bool) -> bool {
    enabled && code == Some(255)
}

pub fn retry_delay(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs(1u64 << attempt.saturating_sub(1).min(5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_identity_but_new_tabs_get_distinct_sessions() {
        let connection = SshConnection::new(SshTarget::parse("user@host", "2222").unwrap(), true);
        let json = serde_json::to_string(&connection).unwrap();
        let restored: SshConnection = serde_json::from_str(&json).unwrap();
        assert_eq!(connection, restored);
        assert_eq!(connection.command(), restored.command());
        assert_ne!(connection.session, connection.new_tab().session);
        assert_eq!(connection.target, connection.new_tab().target);
    }

    #[test]
    fn both_shells_preserve_the_command_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join("ssh");
        std::fs::write(&ssh, "#!/bin/sh\nprintf '%s\\0' \"$@\"\n").unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        let connection = SshConnection::new(
            SshTarget::parse("alice@fe80::1%eth0", "2222").unwrap(),
            true,
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &connection.command()])
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let args: Vec<_> = text.trim_end_matches('\0').split('\0').collect();
        assert_eq!(
            &args[..11],
            [
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ConnectTimeout=10",
                "-t",
                "-p",
                "2222",
                "--",
                "alice@fe80::1%eth0"
            ]
        );
        assert_eq!(args.len(), 12);
        let remote = dir.path().join("tmux");
        std::fs::write(&remote, "#!/bin/sh\nprintf '%s\\0' \"$@\"\n").unwrap();
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", args[11]])
            .env("PATH", dir.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("new-session\0-A\0-s\0{}\0", connection.session)
        );
    }

    #[test]
    fn rejects_untrusted_saved_command_inputs() {
        let connection = SshConnection::new(SshTarget::parse("host", "").unwrap(), true);
        let value = serde_json::to_value(connection).unwrap();
        for (field, bad) in [
            ("destination", "-oProxyCommand=id"),
            ("destination", "host;id"),
            ("session", "limux-';id;#"),
            ("session", "other-session"),
        ] {
            let mut invalid = value.clone();
            invalid[field] = bad.into();
            assert!(serde_json::from_value::<SshConnection>(invalid).is_err());
        }
        let mut invalid = value;
        invalid["port"] = 0.into();
        assert!(serde_json::from_value::<SshConnection>(invalid).is_err());
    }

    #[test]
    fn retries_only_ssh_errors_with_capped_backoff() {
        for code in [None, Some(0), Some(1), Some(127), Some(254), Some(255)] {
            assert!(!should_reconnect(code, false));
            assert_eq!(should_reconnect(code, true), code == Some(255));
        }
        assert_eq!(retry_delay(1).as_secs(), 1);
        assert_eq!(retry_delay(2).as_secs(), 2);
        assert_eq!(retry_delay(u32::MAX).as_secs(), 32);
        assert!(should_reconnect(Some(255), true));
    }
}
