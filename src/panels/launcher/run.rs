/*
 * the launcher's > and >> modes: a typed command run in the background like a
 * keybind, or in a terminal that leaves your shell there afterwards; each
 * mode lists the commands run before (../../run_history.rs) under the typed one
 */

use std::process::Command;
use std::thread;

use amane::spawn;

use crate::panels::list::{self, fuzzy};
use crate::run_history::RunHistory;
use crate::settings::settings;

/*
 * runs the command in the terminal from the settings (foot unless set; its
 * words come before `sh -c ...`, so `alacritty -e` works too), then hands the
 * terminal to your own shell, so the output stays readable above the prompt
 * and the window closes like any other. the command is passed as $1 rather
 * than pasted into the script, so its quotes and $ can't break it; the wrapper
 * ignores ctrl+c, which stops the command but would otherwise close the window
 * too
 */
const IN_TERMINAL: &str = r#"trap : INT; eval "$1"; exec "${SHELL:-/bin/sh}""#;

// where it runs, beside the command in its row
pub fn place(terminal: bool) -> &'static str {
    if terminal {
        "in a terminal"
    } else {
        "background"
    }
}

/*
 * the earlier commands to list: all of them, newest first, while nothing is
 * typed; else the ones matching what's typed, best first (ties newest first),
 * leaving out the typed command itself, which has its own row on top
 */
pub fn matching<'a>(history: &'a [String], typed: &str) -> Vec<&'a String> {
    if typed.is_empty() {
        return history.iter().collect();
    }

    list::ranked(history.iter(), |earlier| {
        if earlier.as_str() == typed {
            None
        } else {
            fuzzy(typed, earlier)
        }
    })
}

pub fn start(command: String, terminal: bool) {
    RunHistory::record(&command, terminal);

    if terminal {
        in_terminal(command);
    } else {
        spawn(&command);
    }
}

fn in_terminal(command: String) {
    thread::spawn(move || {
        let terminal = settings().terminal;
        let mut words = terminal.split_whitespace();
        let program = words.next().unwrap_or_default();

        let started = Command::new(program)
            .args(words)
            .args(["sh", "-c", IN_TERMINAL, "sh"])
            .arg(&command)
            .spawn();

        match started {
            Ok(mut child) => {
                // a finished child nobody waits for stays behind as a zombie
                let _ = child.wait();
            }
            Err(error) => eprintln!("launcher: couldn't start {terminal}: {error}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Vec<String> {
        ["htop", "uname -a", "btop"].map(String::from).to_vec()
    }

    #[test]
    fn nothing_typed_lists_everything_newest_first() {
        let history = history();

        assert_eq!(matching(&history, ""), ["htop", "uname -a", "btop"]);
    }

    #[test]
    fn typing_filters_and_leaves_out_the_typed_command() {
        let history = history();

        assert_eq!(matching(&history, "top"), ["htop", "btop"]);
        assert!(matching(&history, "htop").is_empty());
    }
}
