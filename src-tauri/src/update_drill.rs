//! Breaks a build on purpose, for the update drill (`scripts/update-drill.py`,
//! ADR-089).
//!
//! The drill installs one build over another and checks that a release that
//! cannot start is taken out again. The file [`FILE`] next to `config.toml`,
//! holding `<version>:<how>`, makes the build of that version fail in one of
//! the ways a broken release can; every other version starts as usual, so the
//! version put back is not broken too. Compiled into the drill's builds only
//! (the `update-drill` feature), never into a release.
//!
//! A file and not an environment variable: the Windows installer starts the
//! version it installed through the shell, which does not carry the
//! environment the drill started the first version with.

const FILE: &str = "jamjam-update-drill-break";

/// How far the start has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Nothing of the app has run: the program does not run here at all.
    BeforeStart,
    /// The start has been seen by what decides whether this version stays.
    Started,
    /// The main window's screen has come up, but not for long enough to
    /// count as started.
    ScreenUp,
}

/// Fails the start of `version` here when the drill asked for it.
pub(crate) fn at(stage: Stage, version: &str) {
    let Some(asked) =
        jamjam::config::config_dir().and_then(|dir| std::fs::read_to_string(dir.join(FILE)).ok())
    else {
        return;
    };
    match how(asked.trim(), version, stage) {
        Some(Break::Exit) => std::process::exit(127),
        // What a panic while starting ends as.
        Some(Break::Crash) => std::process::exit(101),
        Some(Break::Hang) => loop {
            std::thread::park();
        },
        None => {}
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Break {
    Exit,
    Crash,
    Hang,
}

fn how(asked: &str, version: &str, stage: Stage) -> Option<Break> {
    let (broken, how) = asked.rsplit_once(':')?;
    if broken != version {
        return None;
    }
    match (how, stage) {
        ("never-runs", Stage::BeforeStart) => Some(Break::Exit),
        ("crash", Stage::Started) => Some(Break::Crash),
        ("hang", Stage::Started) => Some(Break::Hang),
        ("crash-on-screen", Stage::ScreenUp) => Some(Break::Crash),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_version_asked_for_breaks_and_only_where_it_was_asked_to() {
        assert_eq!(
            how("0.1.0-drill.2:crash", "0.1.0-drill.2", Stage::Started),
            Some(Break::Crash)
        );
        assert_eq!(
            how("0.1.0-drill.2:crash", "0.1.0-drill.2", Stage::BeforeStart),
            None
        );
        assert_eq!(
            how("0.1.0-drill.2:crash", "0.1.0-drill.1", Stage::Started),
            None
        );
        assert_eq!(
            how(
                "0.1.0-drill.2:never-runs",
                "0.1.0-drill.2",
                Stage::BeforeStart
            ),
            Some(Break::Exit)
        );
        assert_eq!(
            how("0.1.0-drill.2:hang", "0.1.0-drill.2", Stage::Started),
            Some(Break::Hang)
        );
        assert_eq!(
            how(
                "0.1.0-drill.2:crash-on-screen",
                "0.1.0-drill.2",
                Stage::ScreenUp
            ),
            Some(Break::Crash)
        );
        assert_eq!(
            how(
                "0.1.0-drill.2:crash-on-screen",
                "0.1.0-drill.2",
                Stage::Started
            ),
            None
        );
        assert_eq!(
            how("0.1.0-drill.2:crash", "0.1.0-drill.2", Stage::ScreenUp),
            None
        );
        assert_eq!(
            how("0.1.0-drill.2:sing", "0.1.0-drill.2", Stage::Started),
            None
        );
        assert_eq!(how("nonsense", "0.1.0-drill.2", Stage::Started), None);
    }
}
