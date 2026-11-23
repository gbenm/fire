//! Global flags typed before the first command word (`fire --bg -m "why" api dev`), shared by
//! dispatch and completion so both strip them the same way.

/// Flags shown in help and completion, with their descriptions. `--agent` is internal (MCP) and
/// stays out of this list.
pub(crate) const GLOBAL_FLAGS: [(&str, &str); 9] = [
    ("--bg", "Run the command detached, in a session"),
    (
        "--new",
        "Start a new session even if this command already has one here",
    ),
    ("-m", "Note for the session: why it is running"),
    ("--note", "Note for the session: why it is running"),
    ("--fg", "Run attached in this process, without a session"),
    ("--dry-run", "Print what would run without running it"),
    ("--no-log", "Don't log the commands fire runs"),
    ("-j", "Maximum concurrent dependencies"),
    ("--jobs", "Maximum concurrent dependencies"),
];

/// The flags accepted by the deprecated `fire cli bg`.
pub(crate) const BG_FLAGS: [&str; 3] = ["--new", "-m", "--note"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GlobalFlags {
    pub(crate) bg: bool,
    pub(crate) fg: bool,
    pub(crate) new: bool,
    pub(crate) note: Option<String>,
    pub(crate) dry_run: bool,
    pub(crate) no_log: bool,
    pub(crate) jobs: Option<String>,
    /// Started by an agent (MCP): commands with `agents: false` are refused.
    pub(crate) agent: bool,
}

impl GlobalFlags {
    fn set(&mut self, flag: &str) {
        match flag {
            "--bg" => self.bg = true,
            "--fg" => self.fg = true,
            "--new" => self.new = true,
            "--dry-run" => self.dry_run = true,
            "--no-log" => self.no_log = true,
            "--agent" => self.agent = true,
            _ => {}
        }
    }

    /// Flags that can't go together. Checked separately from parsing so completion can still
    /// read a half-typed line.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.bg && self.fg {
            return Err("--bg and --fg can't be used together".to_string());
        }
        if self.fg && self.note.is_some() {
            return Err("-m/--note needs a session; --fg runs without one".to_string());
        }
        if self.fg && self.new {
            return Err("--new needs a session; --fg runs without one".to_string());
        }
        if let Some(value) = &self.jobs {
            if value
                .parse::<usize>()
                .ok()
                .filter(|jobs| *jobs > 0)
                .is_none()
            {
                return Err(format!("-j/--jobs needs a positive integer, got `{value}`"));
            }
        }
        Ok(())
    }

    /// Hands the flags that have an environment variable to this process and everything it
    /// starts, so nested `fire` calls behave the same (`--dry-run` reaches `fire other` inside
    /// a script).
    pub(crate) fn export(&self) {
        if self.dry_run {
            std::env::set_var("FIRE_DRY_RUN", "true");
        }
        if self.no_log {
            std::env::set_var("FIRE_LOG_COMMANDS", "false");
        }
        if self.fg {
            std::env::set_var("FIRE_SESSIONS", "off");
        }
        if let Some(note) = &self.note {
            std::env::set_var("FIRE_SESSION_NOTE", note);
        }
        if let Some(jobs) = &self.jobs {
            std::env::set_var("FIRE_JOBS", jobs);
        }
    }
}

/// Whether `flag` (exactly, with no value) is a flag fire knows, internal ones included.
fn is_switch(flag: &str) -> bool {
    matches!(
        flag,
        "--bg" | "--fg" | "--new" | "--dry-run" | "--no-log" | "--agent"
    )
}

fn is_note(flag: &str) -> bool {
    flag == "-m" || flag == "--note"
}

fn is_jobs(flag: &str) -> bool {
    flag == "-j" || flag == "--jobs"
}

/// Reads the flags at the start of `args`, stopping at the first word that doesn't start with
/// `-`. Returns the flags and how many words they took.
pub(crate) fn parse(args: &[String]) -> Result<(GlobalFlags, usize), String> {
    let mut flags = GlobalFlags::default();
    let mut index = 0;
    while let Some(word) = args.get(index).map(String::as_str) {
        if !word.starts_with('-') {
            break;
        }
        if is_switch(word) {
            flags.set(word);
            index += 1;
        } else if is_note(word) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("{word} needs a note: {word} \"why it runs\""))?;
            flags.note = Some(value.clone());
            index += 2;
        } else if let Some(value) = word.strip_prefix("--note=") {
            flags.note = Some(value.to_string());
            index += 1;
        } else if is_jobs(word) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("{word} needs a count: {word} <count>"))?;
            flags.jobs = Some(value.clone());
            index += 2;
        } else if let Some(value) = word.strip_prefix("--jobs=") {
            flags.jobs = Some(value.to_string());
            index += 1;
        } else {
            return Err(format!("Unknown flag `{word}`. {}", valid_flags_line()));
        }
    }
    Ok((flags, index))
}

pub(crate) fn valid_flags_line() -> String {
    let names: Vec<&str> = GLOBAL_FLAGS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| *name != "--note" && *name != "--jobs")
        .map(|name| {
            if name == "-m" {
                "-m/--note <text>"
            } else if name == "-j" {
                "-j/--jobs <count>"
            } else {
                name
            }
        })
        .collect();
    format!("Valid flags: {}", names.join(", "))
}

pub(crate) const USAGE: &str = "Usage: fire [--bg|--fg] [--new] [-m \"note\"] [-j <count>] [--dry-run] [--no-log] <command> [args...]";

/// Where completion is within the leading flags.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CompletionPosition {
    /// The word being typed is a flag: offer these (flags not typed yet).
    Flags(Vec<&'static str>),
    /// The word being typed is a note's value: offer nothing.
    NoteValue,
    /// The flags end here: complete `words[n..]` as usual. `n > 0` means flags were typed.
    Command(usize),
}

/// `words` ends with the word being typed. `allowed` limits which flags count (all of
/// `GLOBAL_FLAGS` for `fire`, `BG_FLAGS` for `fire cli bg`).
pub(crate) fn completion_position(
    words: &[String],
    allowed: &[&'static str],
) -> CompletionPosition {
    let Some((current, typed)) = words.split_last() else {
        return CompletionPosition::Command(0);
    };
    let mut flags = GlobalFlags::default();
    let mut index = 0;
    while index < typed.len() {
        let word = typed[index].as_str();
        if is_note(word) {
            if index + 1 == typed.len() {
                return CompletionPosition::NoteValue;
            }
            flags.note = Some(String::new());
            index += 2;
        } else if is_jobs(word) {
            if index + 1 == typed.len() {
                return CompletionPosition::NoteValue;
            }
            flags.jobs = Some(String::new());
            index += 2;
        } else if word.starts_with("--jobs=") {
            flags.jobs = Some(String::new());
            index += 1;
        } else if word.starts_with("--note=") {
            flags.note = Some(String::new());
            index += 1;
        } else if is_switch(word) {
            flags.set(word);
            index += 1;
        } else {
            return CompletionPosition::Command(index);
        }
    }
    if current.starts_with('-') {
        let offered = allowed
            .iter()
            .copied()
            .filter(|flag| flag.starts_with(current.as_str()))
            .filter(|flag| !already_set(&flags, flag) && !conflicts(&flags, flag))
            .collect();
        return CompletionPosition::Flags(offered);
    }
    CompletionPosition::Command(index)
}

fn already_set(flags: &GlobalFlags, flag: &str) -> bool {
    match flag {
        "--bg" => flags.bg,
        "--fg" => flags.fg,
        "--new" => flags.new,
        "--dry-run" => flags.dry_run,
        "--no-log" => flags.no_log,
        "-m" | "--note" => flags.note.is_some(),
        "-j" | "--jobs" => flags.jobs.is_some(),
        _ => false,
    }
}

fn conflicts(flags: &GlobalFlags, flag: &str) -> bool {
    match flag {
        "--bg" => flags.fg,
        "--fg" => flags.bg || flags.new || flags.note.is_some(),
        "--new" | "-m" | "--note" => flags.fg,
        _ => false,
    }
}

/// Reads a boolean environment variable: `1/true/yes/on` or `0/false/no/off`, any case.
/// Unset, empty or anything else is `None`, so the caller's default applies.
pub(crate) fn env_bool(name: &str) -> Option<bool> {
    parse_bool(&std::env::var(name).ok()?)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn flags_combine_in_any_order() {
        let (a, consumed_a) = parse(&words(&[
            "--bg",
            "-m",
            "auth tests",
            "--no-log",
            "api",
            "dev",
        ]))
        .unwrap();
        let (b, consumed_b) = parse(&words(&[
            "--no-log",
            "--bg",
            "--note",
            "auth tests",
            "api",
            "dev",
        ]))
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(consumed_a, 4);
        assert_eq!(consumed_b, 4);
        assert!(a.bg && a.no_log);
        assert_eq!(a.note.as_deref(), Some("auth tests"));
    }

    #[test]
    fn parsing_stops_at_the_first_command_word() {
        let (flags, consumed) = parse(&words(&["--dry-run", "api", "dev", "--bg"])).unwrap();
        assert_eq!(consumed, 1);
        assert!(flags.dry_run && !flags.bg);
        let (flags, consumed) = parse(&words(&["api", "--bg"])).unwrap();
        assert_eq!(consumed, 0);
        assert_eq!(flags, GlobalFlags::default());
    }

    #[test]
    fn repeated_note_keeps_the_last_value() {
        let (flags, _) = parse(&words(&["-m", "one", "--note=two", "--bg", "--bg", "x"])).unwrap();
        assert_eq!(flags.note.as_deref(), Some("two"));
        assert!(flags.bg);
    }

    #[test]
    fn jobs_preserves_its_value_and_requires_a_positive_integer() {
        let (flags, consumed) =
            parse(&words(&["-j", "2", "--jobs=4", "build"])).expect("jobs flags");
        assert_eq!(flags.jobs.as_deref(), Some("4"));
        assert_eq!(consumed, 3);
        assert!(flags.validate().is_ok());
        assert!(parse(&words(&["-j", "build"]))
            .unwrap()
            .0
            .validate()
            .is_err());
        assert!(parse(&words(&["--jobs=0", "build"]))
            .unwrap()
            .0
            .validate()
            .is_err());
    }

    #[test]
    fn note_is_preserved_verbatim() {
        let (flags, _) = parse(&words(&["-m", "  exact note  ", "x"])).unwrap();
        assert_eq!(flags.note.as_deref(), Some("  exact note  "));

        let (flags, _) = parse(&words(&["--note=  exact note  ", "x"])).unwrap();
        assert_eq!(flags.note.as_deref(), Some("  exact note  "));
    }

    #[test]
    fn unknown_flag_and_missing_note_fail() {
        let error = parse(&words(&["--foo", "api"])).unwrap_err();
        assert!(error.contains("Unknown flag `--foo`"), "{error}");
        assert!(error.contains("--dry-run"), "{error}");
        assert!(!error.contains("--agent"), "{error}");
        assert!(parse(&words(&["--bg", "-m"])).is_err());
    }

    #[test]
    fn conflicting_flags_fail_validation() {
        let check = |args: &[&str]| parse(&words(args)).unwrap().0.validate();
        assert!(check(&["--bg", "--fg"]).is_err());
        assert!(check(&["--fg", "-m", "x"]).is_err());
        assert!(check(&["--fg", "--new"]).is_err());
        assert!(check(&["--bg", "--new", "-m", "x", "--dry-run", "--no-log"]).is_ok());
        assert!(check(&["--fg", "--dry-run", "--no-log"]).is_ok());
    }

    #[test]
    fn completion_offers_remaining_flags_without_conflicts() {
        let all: Vec<&str> = GLOBAL_FLAGS.iter().map(|(name, _)| *name).collect();
        match completion_position(&words(&["--bg", "--"]), &all) {
            CompletionPosition::Flags(offered) => {
                assert!(!offered.contains(&"--bg"));
                assert!(!offered.contains(&"--fg"));
                assert!(offered.contains(&"--note"));
                assert!(offered.contains(&"--dry-run"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            completion_position(&words(&["--bg", "-m", ""]), &all),
            CompletionPosition::NoteValue
        );
        assert_eq!(
            completion_position(&words(&["--bg", "-m", "why", ""]), &all),
            CompletionPosition::Command(3)
        );
        assert_eq!(
            completion_position(&words(&["api", "--"]), &all),
            CompletionPosition::Command(0)
        );
    }

    #[test]
    fn boolean_values_are_unified() {
        for value in ["1", "true", "YES", " on "] {
            assert_eq!(parse_bool(value), Some(true), "{value}");
        }
        for value in ["0", "False", "no", "OFF"] {
            assert_eq!(parse_bool(value), Some(false), "{value}");
        }
        assert_eq!(parse_bool(""), None);
        assert_eq!(parse_bool("maybe"), None);
    }
}
