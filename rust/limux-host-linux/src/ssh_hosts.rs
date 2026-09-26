//! Discovery is deliberately passive: never run `ssh -G` (which can execute
//! `Match exec`). OpenSSH resolves the selected alias and all connection options
//! only after the user explicitly connects.

use std::collections::HashSet;

/// A one-shot connection request, not persisted session state.
#[derive(Debug, PartialEq, Eq)]
pub struct SshTarget {
    destination: String,
    port: Option<u16>,
}

impl SshTarget {
    pub fn parse(destination: &str, port: &str) -> Result<Self, String> {
        let destination = destination.trim();
        let (user, host) = match destination.split_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, destination),
        };
        if !valid_host(host) || user.is_some_and(|user| !valid_user(user)) {
            return Err(
                "Enter an SSH alias, hostname, or user@hostname (IPv6 is supported).".into(),
            );
        }
        let port = match port.trim() {
            "" => None,
            value => Some(
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port > 0)
                    .ok_or("Port must be between 1 and 65535.")?,
            ),
        };
        Ok(Self {
            destination: destination.to_string(),
            port,
        })
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn arguments(&self) -> Vec<String> {
        let mut args = vec!["-t".into()];
        if let Some(port) = self.port {
            args.extend(["-p".into(), port.to_string()]);
        }
        args.extend(["--".into(), self.destination.clone()]);
        args
    }

    /// Ghostty's startup API accepts a command string. Quote each argument;
    /// never append remote shell commands or interpolate SSH configuration.
    pub fn command(&self) -> String {
        // Remote hosts need not have Ghostty terminfo installed.
        ["env", "TERM=xterm-256color", "ssh"]
            .into_iter()
            .map(str::to_string)
            .chain(self.arguments())
            .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn valid_user(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-:%".contains(&c))
}

/// List every literal alias in top-level Host directives, preserving order.
/// Wildcards, negations, duplicates and malformed directives are omitted.
/// Include/Match evaluation and option precedence belong to OpenSSH, not this
/// picker. Aliases from included files can still be entered manually.
pub fn parse_ssh_config(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut hosts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let end = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        if !line[..end].eq_ignore_ascii_case("host") {
            continue;
        }
        let value = line[end..].trim_start();
        let value = value.strip_prefix('=').unwrap_or(value).trim_start();
        let Some(aliases) = config_words(value) else {
            continue;
        };
        for alias in aliases {
            if valid_host(&alias) && seen.insert(alias.clone()) {
                hosts.push(alias);
            }
        }
    }
    hosts
}

/// Tokenize Host patterns without interpreting shell expressions.
fn config_words(value: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in value.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else {
            match c {
                '\\' => escaped = true,
                '"' => quoted = !quoted,
                // OpenSSH treats an embedded '#' as part of the alias.
                // Preserve it so validation skips the whole unsupported token.
                '#' if !quoted && word.is_empty() => break,
                c if c.is_whitespace() && !quoted => {
                    if !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                }
                c => word.push(c),
            }
        }
    }
    if quoted || escaped {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

pub fn load_hosts() -> std::io::Result<Vec<String>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    match std::fs::read_to_string(home.join(".ssh/config")) {
        Ok(text) => Ok(parse_ssh_config(&text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_comments_start_only_at_token_boundaries() {
        for (input, expected) in [
            ("foo#bar other # comment", vec!["foo#bar", "other"]),
            ("foo#bar\t# comment", vec!["foo#bar"]),
            ("# comment", vec![]),
            ("foo # comment", vec!["foo"]),
            (r#""foo#bar" other"#, vec!["foo#bar", "other"]),
        ] {
            assert_eq!(
                config_words(input),
                Some(expected.into_iter().map(str::to_string).collect()),
                "{input:?}"
            );
        }
    }

    #[test]
    fn skips_hash_aliases_without_offering_a_different_destination() {
        for directive in [
            "Host foo#bar other",
            "Host=foo#bar other",
            r#"Host "foo#bar" other"#,
        ] {
            let config = format!("{directive}\n  HostName example.invalid\nHost good # comment\n");
            assert_eq!(parse_ssh_config(&config), ["other", "good"], "{directive}");
        }
        assert!(SshTarget::parse("foo#bar", "").is_err());
    }

    #[test]
    fn discovers_all_literal_aliases_without_resolving_options() {
        assert_eq!(
            parse_ssh_config(
                r#"
Host * !excluded
  User default
hOsT = "dev" staging dev *.example ? wildcard*
  HostName ignored.example
  Port 2222
Match exec "touch /tmp/must-not-run"
  HostName also-ignored
Host ipv6-alias # comment
Include extra.conf
Host=last
"#
            ),
            ["dev", "staging", "ipv6-alias", "last"]
        );
    }

    #[test]
    fn rejects_malformed_and_unsafe_aliases() {
        assert_eq!(
            parse_ssh_config(
                "Host -oProxyCommand=oops !no $(oops) a;b user@host\nHost \"unclosed\nHost good\n"
            ),
            ["good"]
        );
    }

    #[test]
    fn command_preserves_alias_and_has_no_remote_command() {
        let target = SshTarget::parse(" dev ", "").unwrap();
        assert_eq!(target.arguments(), ["-t", "--", "dev"]);
        assert_eq!(
            target.command(),
            "'env' 'TERM=xterm-256color' 'ssh' '-t' '--' 'dev'"
        );
    }

    #[test]
    fn explicit_user_port_and_ipv6() {
        let target = SshTarget::parse("alice@fe80::1%eth0", " 2222 ").unwrap();
        assert_eq!(
            target.arguments(),
            ["-t", "-p", "2222", "--", "alice@fe80::1%eth0"]
        );
    }

    #[test]
    fn rejects_option_shell_and_control_injection() {
        for value in [
            "",
            "-oProxyCommand=id",
            "a@-host",
            "-user@host",
            "a@@host",
            "@host",
            "host;id",
            "$(id)",
            "a b",
            "host\nother",
            "host\0",
            "'host'",
            "ssh://host",
            "[::1]",
        ] {
            assert!(SshTarget::parse(value, "").is_err(), "{value:?}");
        }
        for port in ["0", "65536", "-1", "foo", "22 -oProxyCommand=id"] {
            assert!(SshTarget::parse("host", port).is_err(), "{port}");
        }
    }
}
