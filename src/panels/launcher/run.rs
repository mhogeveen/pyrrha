/*
 * the launcher's > and >> modes: a typed command run in the background like a
 * keybind, or in a terminal that leaves your shell there afterwards
 */

use std::process::Command;
use std::thread;

use amane::spawn;

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

pub fn start(command: String, terminal: bool) {
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
