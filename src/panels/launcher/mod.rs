/*
 * app launcher: type to fuzzy-search the installed programs, the ones you use
 * most first; up/down to pick, enter or click to start one
 *
 * what the search starts with switches what it does:
 *
 *   =12*7      calculates, enter copies the answer
 *   >command   runs it in the background, like a keybind
 *   >>command  runs it in a terminal, then leaves your shell there
 *   :wallpaper opens that panel, by name or what it's for
 *   !accent    calls one of the shell's ipc commands, the reply as a notification
 *
 * this file reads the prefix and hands each mode to its own file: apps.rs,
 * run.rs, panel_names.rs and commands.rs; the calculator is ../../calculator.rs
 */

mod apps;
mod commands;
mod panel_names;
mod run;

use std::process::Command;
use std::thread;

use amane::{Apps, DesktopApp, LayerWindow, Monitor, Rectangle, Service};

use super::list;
use super::picker::{self, Picker};
use super::{LAUNCHER, PanelDef, Panels};
use crate::calculator;
use crate::ipc::IpcCommand;
use crate::theme::Theme;
use crate::usage::Usage;

const CALCULATE: &str = "=";
const RUN: &str = ">";
const RUN_IN_TERMINAL: &str = ">>";
const PANEL: &str = ":";
const SHELL_COMMAND: &str = "!";

#[derive(Clone)]
pub enum Entry {
    App(DesktopApp),

    // a calculator's answer, ready to copy
    Answer(String),

    Run {
        command: String,
        terminal: bool,
    },

    Panel(&'static PanelDef),

    // one of the shell's ipc commands, with what was typed after its name
    Ipc {
        command: &'static IpcCommand,
        arguments: Vec<String>,
    },
}

// what the search asks for, by how it starts
enum Mode<'a> {
    Apps(&'a str),
    Calculate(&'a str),
    Run { command: &'a str, terminal: bool },
    Panels(&'a str),
    Ipc(&'a str),
}

fn mode(query: &str) -> Mode<'_> {
    let query = query.trim_start();

    // >> first, it also starts with >
    if let Some(command) = query.strip_prefix(RUN_IN_TERMINAL) {
        Mode::Run {
            command: command.trim(),
            terminal: true,
        }
    } else if let Some(command) = query.strip_prefix(RUN) {
        Mode::Run {
            command: command.trim(),
            terminal: false,
        }
    } else if let Some(sum) = query.strip_prefix(CALCULATE) {
        Mode::Calculate(sum)
    } else if let Some(name) = query.strip_prefix(PANEL) {
        Mode::Panels(name.trim())
    } else if let Some(call) = query.strip_prefix(SHELL_COMMAND) {
        // the space after the last word stays, it starts the next for completion
        Mode::Ipc(call.trim_start())
    } else {
        Mode::Apps(query)
    }
}

pub struct Launcher;

impl Picker for Launcher {
    const PANEL: &'static str = LAUNCHER;
    const INPUT: &'static str = "launcher-search";
    const PLACEHOLDER: &'static str = "Search apps  ·  ? for help";
    const NAMESPACE: &'static str = "amane-launcher";

    type Item = Entry;

    fn items(query: &str) -> Vec<Entry> {
        match mode(query) {
            Mode::Apps(query) => {
                let apps = Apps::read();
                let usage = Usage::read();

                apps::sorted(apps.list(), &usage, query)
                    .into_iter()
                    .map(|app| Entry::App(app.clone()))
                    .collect()
            }

            Mode::Calculate(sum) => calculator::calculate(sum)
                .map(|answer| vec![Entry::Answer(answer)])
                .unwrap_or_default(),

            Mode::Run { command, terminal } if !command.is_empty() => vec![Entry::Run {
                command: String::from(command),
                terminal,
            }],

            Mode::Run { .. } => Vec::new(),

            Mode::Panels(query) => panel_names::matching(query)
                .into_iter()
                .map(Entry::Panel)
                .collect(),

            Mode::Ipc(call) => commands::entries(call),
        }
    }

    fn row(t: &Theme, _: &Monitor, entry: &Entry, selected: bool) -> Rectangle {
        match entry {
            Entry::App(app) => list::row(t, app.name(), selected),

            Entry::Answer(answer) => {
                list::detail_row(t, &format!("= {answer}"), "Enter copies", selected, false)
            }

            Entry::Run { command, terminal } => list::detail_row(
                t,
                &format!("Run {command}"),
                run::place(*terminal),
                selected,
                false,
            ),

            Entry::Panel(panel) => {
                list::detail_row(t, panel.name, panel.description, selected, false)
            }

            Entry::Ipc { command, arguments } => list::detail_row(
                t,
                &commands::call_text(command, arguments),
                commands::row_hint(command),
                selected,
                false,
            ),
        }
    }

    fn pick(entry: Entry) {
        match entry {
            Entry::App(app) => {
                Usage::record(app.name());

                app.launch();
            }

            Entry::Answer(answer) => copy(answer),

            Entry::Run { command, terminal } => run::start(command, terminal),

            // replaces the launcher, which then needs no closing (see picker::choose)
            Entry::Panel(panel) => {
                if let Err(error) = Panels::open(panel.name) {
                    eprintln!("launcher: {error}");
                }
            }

            Entry::Ipc { command, arguments } => commands::call(command, arguments),
        }
    }

    fn help() -> &'static [(&'static str, &'static str)] {
        &[
            ("firefox", "search apps, most used first"),
            ("=12*7", "calculate"),
            (">command", "run in the background"),
            (">>command", "run in a terminal, then your shell"),
            (":wallpaper", "open a panel"),
            ("!accent next", "call a shell command, Tab completes"),
        ]
    }

    fn empty(query: &str) -> &'static str {
        match mode(query) {
            // the first scan walks every icon theme, which takes a few seconds
            Mode::Apps(_) if Apps::read().list().is_empty() => "Loading applications…",
            Mode::Apps(_) => "No matches",

            Mode::Calculate(sum) if sum.trim().is_empty() => "Type a sum, like =12*7",
            Mode::Calculate(sum) => match calculator::calculate(sum) {
                Err(error) => error.message(),
                Ok(_) => "",
            },

            Mode::Run { .. } => "Type a command to run",
            Mode::Panels(_) => "No panel by that name",
            Mode::Ipc(_) => "No command by that name",
        }
    }

    // only ! calls complete; in the other modes Tab does nothing
    fn complete(query: &str, entry: &Entry) -> Option<String> {
        match (mode(query), entry) {
            (Mode::Ipc(_), Entry::Ipc { command, arguments }) => {
                Some(commands::completed(command, arguments))
            }
            _ => None,
        }
    }
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    picker::view::<Launcher>(monitor)
}

pub fn reset() {
    picker::reset::<Launcher>();
}

// the calculator's answer; the clipboard history watcher picks it up from there too
fn copy(answer: String) {
    thread::spawn(move || {
        if let Err(error) = Command::new("wl-copy").arg(answer).status() {
            eprintln!("launcher: couldn't copy the answer: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_start_picks_the_mode() {
        assert!(matches!(mode("firefox"), Mode::Apps("firefox")));
        assert!(matches!(mode("=1+1"), Mode::Calculate("1+1")));
        assert!(matches!(
            mode(">htop"),
            Mode::Run {
                command: "htop",
                terminal: false
            }
        ));
        assert!(matches!(
            mode(">> uname -a"),
            Mode::Run {
                command: "uname -a",
                terminal: true
            }
        ));
    }

    #[test]
    fn colon_lists_panels() {
        assert!(matches!(mode(":wall"), Mode::Panels("wall")));
        assert!(matches!(mode(": wall "), Mode::Panels("wall")));
    }

    #[test]
    fn exclamation_mark_calls_a_command() {
        assert!(matches!(mode("!accent next"), Mode::Ipc("accent next")));
        assert!(matches!(mode(" ! lock "), Mode::Ipc("lock ")));
    }
}
