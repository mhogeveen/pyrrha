/*
 * cover art for the audio panel's "Now playing": players give a file:// path or
 * an https:// link. a path is used as it is; a link is fetched once with curl
 * into $XDG_RUNTIME_DIR/pyrrha/covers, named after the link, so the same
 * cover is never fetched twice (a failed one is tried again after RETRY).
 * the folder is in memory, so only the newest KEEP covers stay there; logging
 * out empties it
 *
 * a view asks with path(), which starts the fetch on a thread the first time;
 * the panel redraws once it's there
 */

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use amane::Service;

use crate::{command, state, wallpaper};

// a cover bigger than this isn't a cover
const MAX_SIZE: &str = "5M";

// a link that failed (offline, say) is fetched again after this, not never
const RETRY: Duration = Duration::from_secs(60);

// curl writes here first, so a fetch cut short by a crash isn't taken for a cover
const PARTIAL: &str = "part";

/*
 * how many covers stay in the folder, newest first: a few albums' worth, while
 * a session that plays music for weeks would otherwise keep every one in memory
 */
const KEEP: usize = 32;

/*
 * the link is a player's (a web page can set it), so curl reads it as one
 * address (no [1-9] or {a,b} ranges) and follows it only over http and https
 */
const SAFE_LINK: [&str; 5] = [
    "--globoff",
    "--proto",
    "=http,https",
    "--proto-redir",
    "=http,https",
];

pub struct Covers {
    // fetched links and the picture each ended up as
    fetched: HashMap<String, PathBuf>,

    // links whose fetch failed, and when
    failed: HashMap<String, Instant>,
}

impl Service for Covers {
    fn new() -> Self {
        Self {
            fetched: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    // changes when a fetch finishes, nothing to poll
    fn listen() {}
}

// links being fetched; not part of the service, so starting one redraws nothing
static FETCHING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/*
 * the picture for a player's art url, None until it's there or when there is
 * none; safe to call from a view
 */
pub fn path(url: &str) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }

    if let Some(path) = file_path(url) {
        return path.is_file().then_some(path);
    }

    if !url.starts_with("https://") && !url.starts_with("http://") {
        return None;
    }

    {
        let covers = Covers::read();

        if let Some(path) = covers.fetched.get(url) {
            return Some(path.clone());
        }

        if covers
            .failed
            .get(url)
            .is_some_and(|failed| failed.elapsed() < RETRY)
        {
            return None;
        }
    }

    fetch(url);

    None
}

// starts one fetch per link
fn fetch(url: &str) {
    {
        let mut fetching = FETCHING.lock().unwrap_or_else(PoisonError::into_inner);

        if !fetching
            .get_or_insert_with(HashSet::new)
            .insert(String::from(url))
        {
            return;
        }
    }

    let url = String::from(url);

    thread::spawn(move || {
        let done = download(&url);

        {
            let mut covers = Covers::write();

            match done {
                Some(path) => {
                    covers.failed.remove(&url);
                    covers.fetched.insert(url.clone(), path);
                }
                None => {
                    covers.failed.insert(url.clone(), Instant::now());
                }
            }

            // covers pruned from the folder, and failures old enough to try again
            covers.fetched.retain(|_, path| path.is_file());
            covers.failed.retain(|_, failed| failed.elapsed() < RETRY);
        }

        if let Some(fetching) = FETCHING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            fetching.remove(&url);
        }
    });
}

fn download(url: &str) -> Option<PathBuf> {
    let folder = state::runtime_path("covers")?;

    fs::create_dir_all(&folder).ok()?;

    let path = folder.join(file_name(url));

    if !path.is_file() {
        let partial = path.with_extension(PARTIAL);

        let fetched = command::capture(
            Command::new("curl")
                .args(["--silent", "--fail", "--location"])
                .args(SAFE_LINK)
                .args(["--max-time", command::CURL_MAX_TIME])
                .args(["--max-filesize", MAX_SIZE])
                .arg("--output")
                .arg(&partial)
                .arg(url),
            command::CURL_TIMEOUT,
        )
        .is_ok_and(|output| output.status.success());

        if !fetched || fs::rename(&partial, &path).is_err() {
            let _ = fs::remove_file(&partial);

            return None;
        }
    } else {
        // one fetched before counts as new again, so pruning keeps it
        let _ = fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(SystemTime::now()));
    }

    // only what amane can draw
    if !wallpaper::is_picture(&path) {
        let _ = fs::remove_file(&path);

        return None;
    }

    prune(&folder);

    Some(path)
}

// removes all but the newest KEEP covers; a fetch still writing its partial file is left alone
fn prune(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };

    let covers: Vec<(PathBuf, SystemTime)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_none() && path.is_file())
        .filter_map(|path| {
            let changed = fs::metadata(&path).and_then(|data| data.modified()).ok()?;

            Some((path, changed))
        })
        .collect();

    for path in beyond_newest(covers, KEEP) {
        let _ = fs::remove_file(path);
    }
}

// the files past the newest `keep`, by change time
fn beyond_newest(mut files: Vec<(PathBuf, SystemTime)>, keep: usize) -> Vec<PathBuf> {
    files.sort_by(|a, b| b.1.cmp(&a.1));

    files.into_iter().skip(keep).map(|(path, _)| path).collect()
}

// the same link always gives the same name
fn file_name(url: &str) -> String {
    let mut hasher = DefaultHasher::new();

    url.hash(&mut hasher);

    format!("{:016x}", hasher.finish())
}

// file:///home/me/My%20Music/cover.jpg is /home/me/My Music/cover.jpg; None for any other link
pub fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;

    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());

        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }

    let path = PathBuf::from(String::from_utf8(decoded).ok()?);

    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_links_are_decoded() {
        assert_eq!(
            file_path("file:///home/me/My%20Music/caf%C3%A9.jpg"),
            Some(PathBuf::from("/home/me/My Music/café.jpg"))
        );
        assert_eq!(
            file_path("file:///tmp/plain.png"),
            Some(PathBuf::from("/tmp/plain.png"))
        );

        // a stray % stays as it is
        assert_eq!(
            file_path("file:///tmp/100%.png"),
            Some(PathBuf::from("/tmp/100%.png"))
        );

        assert_eq!(file_path("https://example.org/cover.jpg"), None);
        assert_eq!(file_path("file://relative/cover.jpg"), None);
    }

    #[test]
    fn only_the_newest_covers_stay() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);

        let files = vec![
            (PathBuf::from("old"), at(10)),
            (PathBuf::from("newest"), at(30)),
            (PathBuf::from("middle"), at(20)),
        ];

        assert_eq!(beyond_newest(files.clone(), 2), vec![PathBuf::from("old")]);
        assert!(beyond_newest(files.clone(), 3).is_empty());
        assert_eq!(beyond_newest(files, 0).len(), 3);
    }

    #[test]
    fn names_follow_the_link() {
        let first = file_name("https://example.org/a.jpg");

        assert_eq!(first, file_name("https://example.org/a.jpg"));
        assert_ne!(first, file_name("https://example.org/b.jpg"));
        assert_eq!(first.len(), 16);
    }
}
