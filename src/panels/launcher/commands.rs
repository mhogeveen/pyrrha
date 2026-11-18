/*
 * the launcher's ! mode: the shell's ipc commands (crate::ipc), found by name,
 * completed with Tab from each command's options, and called once the
 * launcher has gone, the reply as a notification where it's worth one
 */

use std::thread;

use amane::Urgency;

use super::{Entry, SHELL_COMMAND};
use crate::ipc::{self, IpcCommand};
use crate::notifications;
use crate::panels::{self, LAUNCHER, Panels, list};

// who the reply to a ! command comes from
const REPLY_FROM: &str = "Pyrrha";

/*
 * a command's exact name gives the call as typed, with the words that could
 * complete it (see completions); anything else ranks the commands by name, or
 * by what they're for below that, to pick one to call without arguments
 */
pub fn entries(call: &str) -> Vec<Entry> {
    let words: Vec<&str> = call.split_whitespace().collect();
    let first = words.first().copied().unwrap_or_default();

    if let Some(command) = ipc::find(first) {
        return completions(command, &words[1..], call.ends_with(char::is_whitespace));
    }

    // an empty name lists them all; equal scores keep the table's order
    let found = list::ranked(ipc::COMMANDS, |command| {
        if first.is_empty() {
            Some(0)
        } else {
            list::name_or_description(first, command.name, command.description)
        }
    });

    found
        .into_iter()
        .map(|command| Entry::Ipc {
            command,
            arguments: Vec::new(),
        })
        .collect()
}

/*
 * the call as typed, and the calls with the word being typed finished from the
 * command's options. a space after the last word starts the next one, with
 * every option for it; a partial word keeps the options it begins.
 *
 * the call as typed comes first, so Enter runs exactly that, unless a word is
 * being chosen: after a space, or a partial word that isn't itself an option
 * (`!brightness u`), the options come first and Enter takes the top one. `!accent`
 * with nothing after it stays first, it prints the accent in use
 */
fn completions(command: &'static IpcCommand, arguments: &[&str], next_word: bool) -> Vec<Entry> {
    let (before, typing) = match arguments.split_last() {
        Some((last, before)) if !next_word => (before, *last),
        _ => (arguments, ""),
    };

    let options = (command.options)(before);

    let finished: Vec<Entry> = options
        .iter()
        .filter(|option| option.starts_with(typing) && **option != typing)
        .map(|option| {
            let mut words: Vec<String> = before.iter().map(|word| String::from(*word)).collect();

            words.push(String::from(*option));

            Entry::Ipc {
                command,
                arguments: words,
            }
        })
        .collect();

    let typed = Entry::Ipc {
        command,
        arguments: arguments.iter().map(|word| String::from(*word)).collect(),
    };

    let choosing = next_word || (!typing.is_empty() && !options.contains(&typing));

    if choosing && !finished.is_empty() {
        finished.into_iter().chain([typed]).collect()
    } else {
        [typed].into_iter().chain(finished).collect()
    }
}

// how a call reads, in its row and its reply's title
pub fn call_text(command: &IpcCommand, arguments: &[String]) -> String {
    let mut text = format!("{SHELL_COMMAND}{}", command.name);

    for argument in arguments {
        text.push(' ');
        text.push_str(argument);
    }

    text
}

// what Tab fills in: the call up to the selected row's words, with a space for the next one
pub fn completed(command: &IpcCommand, arguments: &[String]) -> String {
    format!("{} ", call_text(command, arguments))
}

// beside the call in its row: what it takes, or what it's for when it takes nothing
pub fn row_hint(command: &IpcCommand) -> &'static str {
    if command.arguments.is_empty() {
        command.description
    } else {
        command.arguments
    }
}

/*
 * closes the launcher first and runs the handler once it's gone, from a
 * thread: a handler may show a surface itself, and one that replaces nothing
 * (`!record menu`'s screenshot menu) would show while the launcher hides,
 * which crashes the shell (see panels::SWAP_DELAY). handlers only write
 * services, which threads may do; the reply waits on the notification daemon,
 * another reason not to block the draw thread. off the draw thread, a handler
 * closing a panel with a search box wouldn't clear it (PanelDef::on_close);
 * none does today, since the launcher has closed by then
 */
pub fn call(command: &'static IpcCommand, arguments: Vec<String>) {
    if let Err(error) = Panels::close(LAUNCHER) {
        eprintln!("launcher: {error}");
    }

    let wait = panels::until_closed();

    thread::spawn(move || {
        thread::sleep(wait);

        let reply = (command.handler)(&arguments);

        // a confirmation of what you can see happen isn't worth a notification
        if !command.notify && !ipc::problem(&reply) {
            return;
        }

        let title = call_text(command, &arguments);

        notifications::send(REPLY_FROM, &title, &reply, Urgency::Normal, None);
    });
}

#[cfg(test)]
mod tests {
    use super::super::Launcher;
    use super::*;
    use crate::panels::picker::Picker;

    // how the rows of a ! search read, top first
    fn ipc_rows(query: &str) -> Vec<String> {
        Launcher::items(query)
            .iter()
            .map(|entry| match entry {
                Entry::Ipc { command, arguments } => call_text(command, arguments),
                _ => panic!("a ! search lists only commands"),
            })
            .collect()
    }

    // what Tab fills in with the top row selected
    fn tab(query: &str) -> Option<String> {
        Launcher::complete(query, &Launcher::items(query)[0])
    }

    #[test]
    fn an_exact_name_takes_the_rest_as_arguments() {
        assert_eq!(
            ipc_rows("!panel  toggle wallpaper"),
            vec!["!panel toggle wallpaper"]
        );
        assert_eq!(
            ipc_rows("!weather location 52.22 5.18 Hilversum"),
            vec!["!weather location 52.22 5.18 Hilversum"]
        );
    }

    #[test]
    fn a_partial_name_ranks_the_commands() {
        assert_eq!(ipc_rows("!bri")[0], "!brightness");
        assert_eq!(tab("!bri").as_deref(), Some("!brightness "));
    }

    #[test]
    fn an_empty_call_lists_every_command_in_order() {
        let all: Vec<String> = ipc::COMMANDS
            .iter()
            .map(|command| format!("!{}", command.name))
            .collect();

        assert_eq!(ipc_rows("!"), all);
    }

    #[test]
    fn a_bare_command_runs_as_typed_before_its_options() {
        let rows = ipc_rows("!accent");

        assert_eq!(rows[0], "!accent");
        assert_eq!(rows[1], "!accent next");
        assert_eq!(tab("!accent").as_deref(), Some("!accent "));
    }

    #[test]
    fn a_space_offers_the_next_word_first() {
        let rows = ipc_rows("!brightness ");

        assert_eq!(
            rows,
            vec!["!brightness up", "!brightness down", "!brightness"]
        );
        assert_eq!(tab("!brightness ").as_deref(), Some("!brightness up "));
    }

    #[test]
    fn a_partial_word_keeps_the_options_it_begins() {
        assert_eq!(
            ipc_rows("!media pl"),
            vec!["!media play-pause", "!media pl"]
        );
        assert_eq!(ipc_rows("!panel toggle wall")[0], "!panel toggle wallpaper");
    }

    #[test]
    fn a_word_that_is_an_option_runs_as_typed() {
        // close also begins close-all, but Enter on `!panel close` shouldn't close everything
        assert_eq!(ipc_rows("!panel close")[0], "!panel close");
        assert_eq!(ipc_rows("!panel close")[1], "!panel close-all");
    }

    #[test]
    fn nothing_left_to_complete_leaves_the_text() {
        // the text already there, which the picker leaves as it is
        assert_eq!(tab("!accent next ").as_deref(), Some("!accent next "));
        // only ! calls complete
        assert_eq!(tab("=12*7"), None);
    }

    #[test]
    fn a_call_reads_as_typed() {
        let accent = ipc::find("accent").unwrap();

        assert_eq!(call_text(accent, &[]), "!accent");
        assert_eq!(call_text(accent, &[String::from("next")]), "!accent next");
    }

    #[test]
    fn a_command_without_arguments_shows_what_its_for() {
        assert_eq!(row_hint(ipc::find("lock").unwrap()), "Lock the screen");
        assert_eq!(row_hint(ipc::find("brightness").unwrap()), "up|down");
    }
}
