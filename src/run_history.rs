/*
 * the commands run from the launcher's > and >> modes, newest first, so a bare
 * > or >> can list them again. each mode keeps its own list, as a command meant
 * for the background is rarely one for a terminal. kept in
 * ~/.local/state/pyrrha/runs and terminal-runs, one command per line
 */

use std::fs;
use std::sync::{Mutex, PoisonError};
use std::thread;

use amane::Service;

use crate::state;

const BACKGROUND_STATE: &str = "runs";
const TERMINAL_STATE: &str = "terminal-runs";

// how many commands each mode remembers; older ones drop off the end
const KEPT: usize = 50;

// held while saving, so saves can't overlap
static SAVING: Mutex<()> = Mutex::new(());

pub struct RunHistory {
    background: Vec<String>,
    terminal: Vec<String>,
}

impl Service for RunHistory {
    // two small files, read the first time > or >> is typed
    fn new() -> Self {
        let read = |name| parse(&fs::read_to_string(state::path(name)).unwrap_or_default());

        Self {
            background: read(BACKGROUND_STATE),
            terminal: read(TERMINAL_STATE),
        }
    }

    // only changes through runs
    fn listen() {}
}

impl RunHistory {
    // newest first
    pub fn commands(&self, terminal: bool) -> &[String] {
        if terminal {
            &self.terminal
        } else {
            &self.background
        }
    }

    // puts the command on top, and saves on a thread so running it never waits on the disk
    pub fn record(command: &str, terminal: bool) {
        {
            let mut history = RunHistory::write();

            let list = if terminal {
                &mut history.terminal
            } else {
                &mut history.background
            };

            remember(list, command);
        }

        /*
         * one save at a time, each writing the list as it is by then, so two
         * quick runs can't finish out of order and save the older list last
         */
        thread::spawn(move || {
            let _saving = SAVING.lock().unwrap_or_else(PoisonError::into_inner);

            let text = serialize(RunHistory::read().commands(terminal));

            let name = if terminal {
                TERMINAL_STATE
            } else {
                BACKGROUND_STATE
            };

            if let Err(error) = state::save(name, &text) {
                eprintln!("run history: couldn't save {name}: {error}");
            }
        });
    }
}

// a command run again moves to the top rather than showing twice
fn remember(list: &mut Vec<String>, command: &str) {
    list.retain(|earlier| earlier != command);
    list.insert(0, String::from(command));
    list.truncate(KEPT);
}

fn parse(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(String::from)
        .take(KEPT)
        .collect()
}

fn serialize(list: &[String]) -> String {
    list.iter()
        // the search box can't hold a newline, but one would split the command in two
        .filter(|command| !command.contains('\n'))
        .map(|command| format!("{command}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeat_moves_to_the_top() {
        let mut list = Vec::new();

        remember(&mut list, "htop");
        remember(&mut list, "btop");
        remember(&mut list, "htop");

        assert_eq!(list, ["htop", "btop"]);
    }

    #[test]
    fn only_the_newest_are_kept() {
        let mut list = Vec::new();

        for number in 0..KEPT + 5 {
            remember(&mut list, &format!("echo {number}"));
        }

        assert_eq!(list.len(), KEPT);
        assert_eq!(list[0], format!("echo {}", KEPT + 4));
    }

    #[test]
    fn the_file_reads_back() {
        let list = vec![String::from("uname -a"), String::from("echo \"$HOME\"")];

        assert_eq!(parse(&serialize(&list)), list);
        assert_eq!(parse("htop\n\n  \nbtop\n"), ["htop", "btop"]);
    }
}
