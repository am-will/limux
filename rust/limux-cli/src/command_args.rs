//! Reading the arguments of one subcommand, after the global options.

use anyhow::{anyhow, bail, Result};

/// Options that take a value in some subcommand.
pub(crate) fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "--workspace"
            | "--tab"
            | "--surface"
            | "--pane"
            | "--target-pane"
            | "--action"
            | "--title"
            | "--url"
            | "--cwd"
            | "--command"
            | "--direction"
            | "--type"
            | "--lines"
            | "--timeout"
            | "--timeout-ms"
            | "--name"
            | "--out"
            | "--subtitle"
            | "--body"
            | "--message"
            | "--event"
            | "--agents"
            | "--selector"
            | "--text"
            | "--attr"
            | "--property"
            | "--value"
            | "--amount"
            | "--unset"
            | "--group"
            | "--color"
    )
}

/// Output options are global, but have always been tolerated after the subcommand.
fn is_output_switch(arg: &str) -> bool {
    matches!(arg, "--json" | "--pretty")
}

/// Whether `--help` or `-h` is an option here, and not a value, text to send or
/// part of a hook command.
pub(crate) fn asks_for_help(command: &str, args: &[String]) -> bool {
    let takes_text = matches!(
        command,
        "send"
            | "send-key"
            | "notify"
            | "rename-workspace"
            | "rename-window"
            | "rename-tab"
            | "create-group"
            | "rename-group"
            | "set-buffer"
            | "display-message"
            | "set-hook"
    );
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "--help" | "-h" => return true,
            "--" => return false,
            _ if option_takes_value(word) => {
                words.next();
            }
            _ if takes_text && !word.starts_with('-') => return false,
            _ => {}
        }
    }
    false
}

/// Separate a subcommand's options from its text. The options in `values` and
/// `switches` count wherever they appear before `--`. From the first other word
/// on, every other word is text and is kept as it is.
///
/// The text is an error when an option has no value, or when a long option that
/// the subcommand does not have comes first: it may be a mistyped target.
pub(crate) fn split_text_args(
    args: &[String],
    values: &[&str],
    switches: &[&str],
) -> (Vec<String>, Result<Option<String>>) {
    let mut options = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut problem = None;
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        let leading = text.is_empty();
        if word == "--" {
            // Leading, it only marks the text. Inside the text, it is part of it.
            text.extend((!leading).then_some(word));
            text.extend(words.by_ref());
        } else if values.contains(&word) || (leading && word == "--id-format") {
            options.push(word.to_string());
            match words.next() {
                Some(value) => options.push(value.to_string()),
                None => problem = Some(format!("{word} requires a value")),
            }
        } else if switches.contains(&word) || (leading && is_output_switch(word)) {
            options.push(word.to_string());
        } else if let Some((name, value)) = inline_value(word, values).filter(|_| leading) {
            options.extend([name.to_string(), value.to_string()]);
        } else if leading && looks_like_option(word) {
            problem = problem.or_else(|| Some(format!("unknown option {word}")));
        } else {
            text.push(word);
        }
    }
    let text = match problem {
        Some(problem) => Err(anyhow!(
            "{problem}; put -- before text that looks like an option"
        )),
        None => Ok((!text.is_empty()).then(|| text.join(" "))),
    };
    (options, text)
}

/// The arguments of a subcommand that acts on the caller's own terminal when it
/// is given no target. One it does not know must not leave it with that default.
pub(crate) fn checked_args(
    command: &str,
    args: &[String],
    values: &[&str],
    switches: &[&str],
    max_words: usize,
) -> Result<Vec<String>> {
    let mut checked = Vec::new();
    let mut words = 0;
    let mut rest = args.iter().map(String::as_str);
    while let Some(arg) = rest.next() {
        let option = if values.contains(&arg) {
            let value = rest
                .next()
                .ok_or_else(|| anyhow!("{arg} requires a value"))?;
            Some((arg, value))
        } else {
            inline_value(arg, values)
        };
        if let Some((name, value)) = option {
            if name != "--command" && (value.trim().is_empty() || value.starts_with("--")) {
                bail!("{name} requires a non-empty value");
            }
            checked.extend([name.to_string(), value.to_string()]);
        } else if switches.contains(&arg) || is_output_switch(arg) {
            checked.push(arg.to_string());
        } else if arg == "--id-format" {
            let value = rest
                .next()
                .ok_or_else(|| anyhow!("--id-format requires a value"))?;
            if value.trim().is_empty() || value.starts_with('-') {
                bail!("--id-format requires a non-empty value");
            }
        } else if let Some((_, value)) = inline_value(arg, &["--id-format"]) {
            if value.trim().is_empty() || value.starts_with('-') {
                bail!("--id-format requires a non-empty value");
            }
        } else if !arg.starts_with('-') && words < max_words {
            words += 1;
            checked.push(arg.to_string());
        } else {
            bail!("{command} does not take {arg:?}; run `limux {command} --help`");
        }
    }
    Ok(checked)
}

/// `--name=value` for a name in `values`.
fn inline_value<'a>(arg: &'a str, values: &[&str]) -> Option<(&'a str, &'a str)> {
    arg.split_once('=')
        .filter(|(name, _)| values.contains(name))
}

fn looks_like_option(word: &str) -> bool {
    word.strip_prefix("--").is_some_and(|name| {
        name.starts_with(|ch: char| ch.is_ascii_alphanumeric())
            && !name.contains(char::is_whitespace)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    fn send_args(args: &[&str]) -> (Vec<String>, Result<Option<String>>) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        split_text_args(&args, &["--workspace", "--surface"], &["--osc"])
    }

    #[test]
    fn text_keeps_every_word_that_is_not_an_option_of_the_command() {
        for (line, options, text) in [
            ("rm -i file", "", "rm -i file"),
            ("--surface 4:a ls -la", "--surface 4:a", "ls -la"),
            ("git commit -m fix --amend", "", "git commit -m fix --amend"),
            ("echo --name bob --json", "", "echo --name bob --json"),
            ("-la", "", "-la"),
            ("- item one", "", "- item one"),
            ("git checkout -- file", "", "git checkout -- file"),
            // Target options are honored on both sides of the text.
            ("hi --surface 4:a", "--surface 4:a", "hi"),
            (
                "--workspace w hi --surface 4:a there",
                "--workspace w --surface 4:a",
                "hi there",
            ),
            ("--surface=4:a hi", "--surface 4:a", "hi"),
            ("hi --osc", "--osc", "hi"),
            ("--json hi", "--json", "hi"),
            // Everything after -- is text.
            ("-- --surface 4:a", "", "--surface 4:a"),
            (
                "--surface 4:a -- cargo test --workspace w",
                "--surface 4:a",
                "cargo test --workspace w",
            ),
            ("echo -- --surface 4:a", "", "echo -- --surface 4:a"),
        ] {
            let (found, found_text) =
                send_args(&words(line).iter().map(String::as_str).collect::<Vec<_>>());
            assert_eq!(found.join(" "), options, "{line}");
            assert_eq!(found_text.unwrap().as_deref(), Some(text), "{line}");
        }
        assert_eq!(send_args(&["--surface", "4:a"]).1.unwrap(), None);
        assert_eq!(
            send_args(&["- one\n- two"]).1.unwrap().as_deref(),
            Some("- one\n- two")
        );
    }

    #[test]
    fn text_rejects_unknown_leading_options_and_missing_values() {
        for args in [
            &["--surfce", "4:a", "hi"][..],
            &["--pane", "5", "hi"],
            &["cargo", "build", "--workspace"],
        ] {
            let error = send_args(args).1.unwrap_err().to_string();
            assert!(error.contains("put -- before text"), "{error}");
        }
    }

    #[test]
    fn checked_args_reject_what_the_command_would_ignore() {
        let check = |line: &str| {
            checked_args(
                "close-surface",
                &words(line),
                &["--workspace", "--surface"],
                &[],
                0,
            )
        };
        assert_eq!(
            check("--surface=4:a --workspace w --json").unwrap(),
            words("--surface 4:a --workspace w --json")
        );
        for line in [
            "4:a",
            "--surface",
            "--surface=",
            "--surface --workspace w",
            "--pane 5",
            "-t 4:a",
            "--surface 4:a 5:b",
            "--id-format",
            "--id-format=",
            "--id-format --surface 4:a",
            "--id-format -x",
            "--id-format=--surface",
        ] {
            assert!(check(line).is_err(), "{line}");
        }
        assert_eq!(
            check("--id-format refs --surface 4:a").unwrap(),
            words("--surface 4:a")
        );
        assert_eq!(
            check("--id-format=uuids --surface 4:a").unwrap(),
            words("--surface 4:a")
        );
    }

    #[test]
    fn checked_args_require_a_command_value_when_omitted_but_allow_any_supplied_value() {
        let check = |args: &[&str]| {
            let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            checked_args(
                "respawn-pane",
                &owned,
                &["--workspace", "--surface", "--command"],
                &["-k"],
                0,
            )
        };
        let error = check(&["--command"]).unwrap_err().to_string();
        assert_eq!(error, "--command requires a value");
        assert_eq!(check(&["--command", ""]).unwrap(), vec!["--command", ""]);
        assert_eq!(
            check(&["--command", "--foo"]).unwrap(),
            vec!["--command", "--foo"]
        );
        assert_eq!(check(&["--command="]).unwrap(), vec!["--command", ""]);
        assert_eq!(
            check(&["--command=--foo"]).unwrap(),
            vec!["--command", "--foo"]
        );
    }
}
