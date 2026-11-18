/*
 * every ipc command the shell answers, in one table: main registers each with
 * amane, and the launcher's ! mode lists, completes and calls them. amane
 * keeps its handlers in a private map it can't list, so this table is the
 * only place a command is added
 */

use crate::brightness;
use crate::eye_breaks;
use crate::osd;
use crate::panels;
use crate::recording;
use crate::session;
use crate::theme::{self, ACCENTS};
use crate::weather;

pub struct IpcCommand {
    pub name: &'static str,
    pub handler: fn(&[String]) -> String,

    // what it takes after the name, shown beside it in the launcher
    pub arguments: &'static str,

    // what it's for, which the launcher's search also matches
    pub description: &'static str,

    /*
     * the words that can come next, given the arguments before it, for the
     * launcher's tab completion; empty where anything goes, like a place name
     */
    pub options: fn(&[&str]) -> Vec<&'static str>,

    /*
     * whether the launcher shows every reply as a notification: for commands
     * whose reply is something to read (the accent in use, the weather place);
     * the rest act where you can see it, so only a problem shows (see problem)
     */
    pub notify: bool,
}

pub static COMMANDS: &[IpcCommand] = &[
    IpcCommand {
        name: "panel",
        handler: panels::ipc,
        arguments: "open|close|toggle <panel> · close-all · list",
        description: "Open or close a panel",
        options: panel_options,
        notify: false,
    },
    IpcCommand {
        name: "lock",
        handler: session::lock_ipc,
        arguments: "",
        description: "Lock the screen",
        options: no_options,
        notify: false,
    },
    IpcCommand {
        name: "accent",
        handler: theme::accent_ipc,
        arguments: "[next|<accent>]",
        description: "Show or change the accent color",
        options: accent_options,
        notify: true,
    },
    IpcCommand {
        name: "weather",
        handler: weather::ipc,
        arguments: "[refresh|location <lat> <lon> <name>]",
        description: "Weather place and refresh",
        options: weather_options,
        notify: true,
    },
    IpcCommand {
        name: "media",
        handler: osd::media_ipc,
        arguments: "play-pause|next|previous",
        description: "Control the active player",
        options: media_options,
        notify: false,
    },
    IpcCommand {
        name: "brightness",
        handler: brightness::ipc,
        arguments: "up|down",
        description: "Monitor brightness",
        options: brightness_options,
        notify: false,
    },
    IpcCommand {
        name: "record",
        handler: recording::ipc,
        arguments: "stop|menu",
        description: "Stop a recording or toggle the screenshot menu",
        options: record_options,
        notify: false,
    },
    IpcCommand {
        name: "eye-breaks",
        handler: eye_breaks::ipc,
        arguments: "skip",
        description: "Skip the eye break on screen",
        options: eye_breaks_options,
        notify: false,
    },
];

pub fn find(name: &str) -> Option<&'static IpcCommand> {
    COMMANDS.iter().find(|command| command.name == name)
}

/*
 * how the handlers' replies that say something went wrong begin; handlers
 * answer `amane ipc call` with text alone, so this is how the launcher tells a
 * problem, which it always shows, from a confirmation it may leave out. the
 * tests call each handler wrongly, so a new kind of complaint fails there first
 */
const PROBLEMS: [&str; 6] = [
    "usage:",
    "unknown ",
    "no player",
    "not recording",
    "no break",
    "latitude ",
];

pub fn problem(reply: &str) -> bool {
    PROBLEMS.iter().any(|start| reply.starts_with(start))
}

fn no_options(_: &[&str]) -> Vec<&'static str> {
    Vec::new()
}

// the first word only, from a fixed list
fn first_of(before: &[&str], words: &[&'static str]) -> Vec<&'static str> {
    if before.is_empty() {
        words.to_vec()
    } else {
        Vec::new()
    }
}

// a panel's name after open, close or toggle; the same panels the launcher's : lists
fn panel_options(before: &[&str]) -> Vec<&'static str> {
    match before {
        [] => vec!["open", "close", "toggle", "close-all", "list"],
        ["open" | "close" | "toggle"] => panels::launchable().map(|panel| panel.name).collect(),
        _ => Vec::new(),
    }
}

fn accent_options(before: &[&str]) -> Vec<&'static str> {
    if !before.is_empty() {
        return Vec::new();
    }

    let mut words = vec!["next"];

    words.extend(ACCENTS.iter().map(|accent| accent.name()));

    words
}

fn weather_options(before: &[&str]) -> Vec<&'static str> {
    first_of(before, &["refresh", "location"])
}

fn media_options(before: &[&str]) -> Vec<&'static str> {
    first_of(before, &["play-pause", "next", "previous"])
}

fn brightness_options(before: &[&str]) -> Vec<&'static str> {
    first_of(before, &["up", "down"])
}

fn record_options(before: &[&str]) -> Vec<&'static str> {
    first_of(before, &["stop", "menu"])
}

fn eye_breaks_options(before: &[&str]) -> Vec<&'static str> {
    first_of(before, &["skip"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_its_own_name() {
        for (index, command) in COMMANDS.iter().enumerate() {
            assert!(
                COMMANDS[index + 1..]
                    .iter()
                    .all(|other| other.name != command.name),
                "{} is in the table twice",
                command.name
            );
        }
    }

    #[test]
    fn commands_are_found_by_exact_name() {
        assert_eq!(find("accent").map(|command| command.name), Some("accent"));
        assert!(find("acc").is_none());
    }

    #[test]
    fn panel_names_follow_a_panel_action() {
        let actions = (find("panel").unwrap().options)(&[]);
        let names = (find("panel").unwrap().options)(&["toggle"]);

        assert!(actions.contains(&"close-all"));
        assert!(names.contains(&panels::WALLPAPER));
        assert!((find("panel").unwrap().options)(&["list"]).is_empty());
        assert!((find("panel").unwrap().options)(&["toggle", "audio"]).is_empty());
    }

    #[test]
    fn accents_are_offered_by_name() {
        let words = (find("accent").unwrap().options)(&[]);

        assert_eq!(words[0], "next");
        assert!(words.contains(&ACCENTS[0].name()));
        assert!((find("accent").unwrap().options)(&["next"]).is_empty());
    }

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    // wrong calls only: none of these change anything or read a running service
    #[test]
    fn wrong_calls_reply_with_a_problem() {
        let wrong = [
            ("panel", ""),
            ("panel", "open"),
            ("panel", "open nowhere"),
            ("accent", "nope"),
            ("weather", "nope"),
            ("weather", "location north east"),
            ("weather", "location 99 0"),
            ("media", "nope"),
            ("brightness", "nope"),
            ("record", "nope"),
            ("eye-breaks", "nope"),
            // no cycle runs in tests, so this answers before reading the service
            ("eye-breaks", "skip"),
        ];

        for (name, arguments) in wrong {
            let reply = (find(name).unwrap().handler)(&words(arguments));

            assert!(problem(&reply), "{name} {arguments}: {reply}");
        }
    }

    #[test]
    fn confirmations_are_no_problem() {
        for reply in [
            "opened", "closed", "up", "next", "frost", "stopping", "locked", "skipped",
        ] {
            assert!(!problem(reply), "{reply}");
        }
    }

    #[test]
    fn only_replies_worth_reading_always_notify() {
        let notifying: Vec<&str> = COMMANDS
            .iter()
            .filter(|command| command.notify)
            .map(|command| command.name)
            .collect();

        assert_eq!(notifying, vec!["accent", "weather"]);
    }

    #[test]
    fn free_arguments_offer_nothing() {
        assert!((find("weather").unwrap().options)(&["location"]).is_empty());
        assert!((find("lock").unwrap().options)(&[]).is_empty());
    }
}
