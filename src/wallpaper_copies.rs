/*
 * ready-sized copies of the wallpapers, made with ffmpeg (the resizer
 * setting) in $XDG_CACHE_HOME/pyrrha/wallpapers, so amane only ever decodes
 * small files: a tile for the picker (its THUMBNAIL size) and one per
 * monitor size for the background, at the monitor's physical pixels
 *
 * why: amane decodes every picture at full size before it shrinks it, on a
 * thread per picture with no limit, and keeps a shrunk copy until the shell
 * exits however big it is (docs/amane.md). opening the picker decoded every
 * wallpaper at once (605 MB at the peak), and every previewed wallpaper
 * stayed in memory. ffmpeg decodes one file at a time in its own process,
 * which gives the memory back when it exits
 *
 * the background copy is lanczos-scaled straight from the original to just
 * cover the monitor's pixels and kept as a lossless png, so it's drawn 1:1;
 * a picture no bigger than that is used as it is, never scaled up. a copy is
 * named after its picture's path, size and change time, so an edited picture
 * gets new copies and the old ones are removed
 *
 * the work is done on this service's own thread (listen): a pass finds the
 * copies already made, then makes the missing ones one by one, the
 * wallpaper shown first, then the tiles, then the other backgrounds. a pass
 * runs at start and whenever the picker reads the folder again (refresh).
 * without ffmpeg, or when mango doesn't say its monitors, the views do what
 * they did before (Prepared::Unavailable)
 */

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use amane::Service;

use crate::command::{self, Failure};
use crate::json::{self, Value};
use crate::panels::wallpaper::THUMBNAIL;
use crate::settings::settings;
use crate::state;
use crate::wallpaper::{self, Wallpaper};

// the copies' folder, in the shell's cache folder
const FOLDER: &str = "wallpapers";

// what a finished copy and one still being written end in
const EXTENSION: &str = "png";
const PARTIAL_EXTENSION: &str = "part.png";

/*
 * how much of a picture is read to find its size: a png says it in its first
 * 24 bytes, a jpeg after its metadata, which a camera can make long
 */
const FIRST_READ: usize = 64 * 1024;
const LONGEST_READ: usize = 1024 * 1024;

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const PNG_SIZE_AT: usize = 16;

const JPEG: &[u8] = &[0xFF, 0xD8];
const JPEG_MARKER: u8 = 0xFF;

// mango answers at once; a stuck mmsg shouldn't hold up the first frame
const MMSG_TIMEOUT: Duration = Duration::from_secs(2);

// `ffmpeg -version`, to know whether there's an ffmpeg at all
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

// one copy; a big png on one thread at the lowest priority takes a few seconds
const COPY_TIMEOUT: Duration = Duration::from_secs(120);

/*
 * the shell's own LD_LIBRARY_PATH holds libraries built against another glibc
 * (alsa-lib), which ffmpeg fails to load, like wf-recorder (src/recording.rs)
 */
const LIBRARY_PATH: &str = "LD_LIBRARY_PATH";

// the gentlest priority, so making copies never slows what's on screen
const NICENESS: &str = "19";

// lanczos, with exact rounding and full colour resolution, for the sharpest copy
const SCALE_FLAGS: &str = "lanczos+accurate_rnd+full_chroma_int+full_chroma_inp";

// width and height in pixels
pub type Size = (u32, u32);

pub struct Copies {
    resizer: Resizer,

    // each monitor's physical size by name, from mango
    screens: HashMap<String, Size>,

    // the file to draw for a picture at a size: its copy, or the picture itself when it's small enough
    ready: HashMap<(PathBuf, Size), PathBuf>,

    // copies ffmpeg couldn't make, or pictures whose size couldn't be read
    failed: HashSet<(PathBuf, Size)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resizer {
    // not looked for yet
    Unknown,
    Missing,
    Present,
}

// what a view draws for a picture
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    // this file, already the right size
    Ready(PathBuf),

    // nothing yet: the copy is being made, and the original is what the copies avoid decoding
    Waiting,

    // no copy will come (no ffmpeg, an unknown monitor, a file ffmpeg can't read): do as before
    Unavailable,
}

impl Service for Copies {
    /*
     * the wallpaper in use is looked up at once, so the first frame after a
     * start draws its copy instead of waiting on the first pass; the rest is
     * found by that pass. reads the saved choice, not the Wallpaper service,
     * since a view may be holding that one
     */
    fn new() -> Self {
        let screens = monitor_sizes();
        let folder = folder();

        let mut ready = HashMap::new();

        if let Some(current) = wallpaper::saved_choice()
            && let Some(source) = Source::read(&current)
        {
            // its backgrounds, and its tile, which the lock screen's warm-up waits on
            for &size in screens.values().chain([&THUMBNAIL]) {
                if let Some(file) = source.existing(&current, size, &folder) {
                    ready.insert((current.clone(), size), file);
                }
            }
        }

        Self {
            resizer: Resizer::Unknown,
            screens,
            ready,
            failed: HashSet::new(),
        }
    }

    // a pass at start, then one each time refresh asks
    fn listen() {
        loop {
            pass();

            wait_for_refresh();
        }
    }
}

impl Copies {
    fn prepared(&self, picture: &Path, size: Size) -> Prepared {
        let key = (picture.to_path_buf(), size);

        if let Some(file) = self.ready.get(&key) {
            return Prepared::Ready(file.clone());
        }

        if self.failed.contains(&key) {
            return Prepared::Unavailable;
        }

        match self.resizer {
            Resizer::Missing => Prepared::Unavailable,
            Resizer::Unknown | Resizer::Present => Prepared::Waiting,
        }
    }
}

// a monitor's background: the picture at the monitor's physical size; safe inside views
pub fn screen(picture: &Path, monitor: &str) -> Prepared {
    let copies = Copies::read();

    match copies.screens.get(monitor) {
        Some(&size) => copies.prepared(picture, size),
        None => Prepared::Unavailable,
    }
}

// the picker's tile, also the source of the lock screen's blur; safe inside views
pub fn tile(picture: &Path) -> Prepared {
    Copies::read().prepared(picture, THUMBNAIL)
}

// asks for another pass, after the picker read the folder again; not from inside a view
pub fn refresh() {
    let (requested, wake) = &REFRESH;

    *requested.lock().unwrap_or_else(PoisonError::into_inner) = true;

    wake.notify_one();
}

// whether a pass was asked for, and the worker waiting on it
static REFRESH: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn wait_for_refresh() {
    let (requested, wake) = &REFRESH;

    let mut requested = requested.lock().unwrap_or_else(PoisonError::into_inner);

    while !*requested {
        requested = wake.wait(requested).unwrap_or_else(PoisonError::into_inner);
    }

    *requested = false;
}

// $XDG_CACHE_HOME/pyrrha/wallpapers
fn folder() -> PathBuf {
    state::cache_path(FOLDER)
}

/*
 * finds what's made, tells the views, removes what's no longer needed, then
 * makes the rest one at a time
 */
fn pass() {
    let present = resizer_present();
    let screens = monitor_sizes();

    {
        let mut copies = Copies::write();

        copies.resizer = if present {
            Resizer::Present
        } else {
            Resizer::Missing
        };

        copies.screens = screens.clone();
    }

    if !present {
        return;
    }

    let folder = folder();

    let mut sizes: Vec<Size> = screens.values().copied().collect();
    sizes.push(THUMBNAIL);
    sizes.sort_unstable();
    sizes.dedup();

    let mut sources = wallpaper::pictures(&wallpaper::folder()).unwrap_or_default();

    // the one in use, also when it's from another folder; the read ends with the statement
    let current = Wallpaper::read().current().map(PathBuf::from);

    if let Some(current) = current
        && !sources.contains(&current)
    {
        sources.push(current);
    }

    let mut ready = HashMap::new();
    let mut failed = HashSet::new();
    let mut wanted = HashSet::new();
    let mut jobs = Vec::new();

    for picture in &sources {
        let source = Source::read(picture);

        for &size in &sizes {
            let key = (picture.clone(), size);

            let Some(source) = &source else {
                failed.insert(key);
                continue;
            };

            if !source.needs_copy(size) {
                ready.insert(key, picture.clone());
                continue;
            }

            let output = folder.join(source.copy_name(picture, size));

            wanted.insert(output.clone());

            if output.is_file() {
                ready.insert(key, output);
            } else {
                jobs.push(Job {
                    picture: picture.clone(),
                    size,
                    output,
                });
            }
        }
    }

    {
        let mut copies = Copies::write();

        copies.ready = ready;
        copies.failed = failed;
    }

    // without mango's sizes the background copies would all look unwanted
    if !screens.is_empty() {
        remove_unwanted(&folder, &wanted);
    }

    if fs::create_dir_all(&folder).is_err() {
        eprintln!("wallpaper copies: couldn't make {}", folder.display());

        return;
    }

    let tile = THUMBNAIL;

    while !jobs.is_empty() {
        let shown = Wallpaper::read().shown().map(PathBuf::from);

        let job = jobs.remove(next_job(&jobs, shown.as_deref(), tile));

        let key = (job.picture.clone(), job.size);

        if make(&job) {
            Copies::write().ready.insert(key, job.output);
        } else {
            Copies::write().failed.insert(key);
        }
    }
}

// a copy still to make
#[derive(Debug, Clone, PartialEq)]
struct Job {
    picture: PathBuf,
    size: Size,
    output: PathBuf,
}

/*
 * the shown wallpaper's background first (the screen is waiting on it), then
 * the tiles (the picker is), then the other backgrounds; in list order otherwise
 */
fn next_job(jobs: &[Job], shown: Option<&Path>, tile: Size) -> usize {
    let shown_background = jobs
        .iter()
        .position(|job| job.size != tile && Some(job.picture.as_path()) == shown);

    let first_tile = || jobs.iter().position(|job| job.size == tile);

    shown_background.or_else(first_tile).unwrap_or(0)
}

/*
 * runs ffmpeg into a file beside the copy and moves it over when it's done, so
 * a shell stopped halfway never leaves a cut-off copy that looks finished
 */
fn make(job: &Job) -> bool {
    let partial = job.output.with_extension(PARTIAL_EXTENSION);

    let mut words = settings().resizer.split_whitespace();
    let program = words.next().unwrap_or_default();

    let mut ffmpeg = Command::new("nice");

    ffmpeg
        .args(["-n", NICENESS, program])
        .args(words)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-threads", "1", "-filter_threads", "1"])
        .arg("-i")
        .arg(&job.picture)
        .args(["-vf", &filter(job.size)])
        .args(["-frames:v", "1", "-update", "1"])
        .arg(&partial)
        .env_remove(LIBRARY_PATH);

    let made = match command::capture(&mut ffmpeg, COPY_TIMEOUT) {
        Ok(output) if output.status.success() && partial.is_file() => {
            fs::rename(&partial, &job.output).is_ok()
        }
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr);

            eprintln!(
                "wallpaper copies: ffmpeg couldn't copy {}: {}",
                job.picture.display(),
                error.trim()
            );

            false
        }
        Err(Failure::Start) => {
            eprintln!("wallpaper copies: couldn't start {program}");

            false
        }
        Err(Failure::Timeout) => {
            eprintln!(
                "wallpaper copies: {} took too long, skipped",
                job.picture.display()
            );

            false
        }
    };

    if !made {
        let _ = fs::remove_file(&partial);
    }

    made
}

/*
 * the picture scaled to just cover the size, keeping its shape; the tile is
 * cut to the size too, the background left whole for amane to place
 */
fn filter((width, height): Size) -> String {
    let scale =
        format!("scale={width}:{height}:force_original_aspect_ratio=increase:flags={SCALE_FLAGS}");

    if (width, height) == THUMBNAIL {
        format!("{scale},crop={width}:{height}")
    } else {
        scale
    }
}

fn resizer_present() -> bool {
    let mut words = settings().resizer.split_whitespace();

    let Some(program) = words.next() else {
        return false;
    };

    command::read(
        Command::new(program)
            .arg("-version")
            .env_remove(LIBRARY_PATH),
        PROBE_TIMEOUT,
    )
    .is_some()
}

// every monitor's physical size, by name; empty when mango doesn't answer
fn monitor_sizes() -> HashMap<String, Size> {
    command::read(
        Command::new("mmsg").args(["get", "all-monitors"]),
        MMSG_TIMEOUT,
    )
    .and_then(|answer| json::parse(&answer))
    .map(|monitors| physical_sizes(&monitors))
    .unwrap_or_default()
}

// mango gives the size in logical pixels and the scale; the copy wants the pixels on the glass
fn physical_sizes(monitors: &Value) -> HashMap<String, Size> {
    monitors
        .get("monitors")
        .list()
        .iter()
        .filter_map(|monitor| {
            let name = monitor.get("name").text();
            let width = monitor.get("width").number()?;
            let height = monitor.get("height").number()?;
            let scale = monitor.get("scale").number().unwrap_or(1.0);

            let size = (
                (width * scale).round() as u32,
                (height * scale).round() as u32,
            );

            (!name.is_empty() && size.0 > 0 && size.1 > 0).then(|| (String::from(name), size))
        })
        .collect()
}

// removes copies and leftover partial files that no picture needs anymore
fn remove_unwanted(folder: &Path, wanted: &HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };

    let files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();

    for file in unwanted(&files, wanted) {
        let _ = fs::remove_file(file);
    }
}

// only pngs are touched, the copies' own kind; anything else in the folder is left alone
fn unwanted<'a>(files: &'a [PathBuf], wanted: &HashSet<PathBuf>) -> Vec<&'a PathBuf> {
    files
        .iter()
        .filter(|file| {
            file.extension()
                .is_some_and(|extension| extension == EXTENSION)
        })
        .filter(|file| !wanted.contains(*file))
        .collect()
}

// what a copy needs to know about its picture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Source {
    width: u32,
    height: u32,

    // the path, length and change time, mixed; changes when the file does
    stamp: u64,
}

impl Source {
    // None for a file that can't be read, or isn't a png or jpeg amane could show
    fn read(picture: &Path) -> Option<Self> {
        let metadata = fs::metadata(picture).ok()?;

        let changed = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let (width, height) = picture_size(picture)?;

        let mut stamp = Stamp::new();
        stamp.add(picture.as_os_str().as_bytes());
        stamp.add(&metadata.len().to_le_bytes());
        stamp.add(&changed.as_secs().to_le_bytes());
        stamp.add(&changed.subsec_nanos().to_le_bytes());

        Some(Self {
            width,
            height,
            stamp: stamp.finish(),
        })
    }

    /*
     * a copy only when the picture is bigger than the size both ways; the
     * same rule amane shrinks by, so a smaller picture is never scaled up
     */
    fn needs_copy(&self, (width, height): Size) -> bool {
        self.width > width && self.height > height
    }

    // like fuji-0a1b2c3d4e5f6789-3840x2160.png
    fn copy_name(&self, picture: &Path, (width, height): Size) -> String {
        let name = wallpaper::name(picture);

        format!("{name}-{:016x}-{width}x{height}.{EXTENSION}", self.stamp)
    }

    // the file to draw when it's there already: the picture itself, or a copy made before
    fn existing(&self, picture: &Path, size: Size, folder: &Path) -> Option<PathBuf> {
        if !self.needs_copy(size) {
            return Some(picture.to_path_buf());
        }

        let copy = folder.join(self.copy_name(picture, size));

        copy.is_file().then_some(copy)
    }
}

// fnv-1a, 64 bit: enough to tell a picture's versions apart, no crates needed
struct Stamp(u64);

impl Stamp {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn add(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

// a picture's size from its first bytes; a longer read only for a jpeg with long metadata
fn picture_size(picture: &Path) -> Option<Size> {
    let head = read_start(picture, FIRST_READ)?;

    if let Some(size) = size_from_header(&head) {
        return Some(size);
    }

    if head.starts_with(JPEG) && head.len() == FIRST_READ {
        return size_from_header(&read_start(picture, LONGEST_READ)?);
    }

    None
}

fn read_start(picture: &Path, length: usize) -> Option<Vec<u8>> {
    let file = File::open(picture).ok()?;

    let mut bytes = Vec::with_capacity(length);

    file.take(length as u64).read_to_end(&mut bytes).ok()?;

    Some(bytes)
}

fn size_from_header(bytes: &[u8]) -> Option<Size> {
    if bytes.starts_with(PNG) {
        return png_size(bytes);
    }

    if bytes.starts_with(JPEG) {
        return jpeg_size(bytes);
    }

    None
}

// the IHDR chunk always comes first: width and height, big-endian, from byte 16
fn png_size(bytes: &[u8]) -> Option<Size> {
    let width = big_endian_u32(bytes.get(PNG_SIZE_AT..PNG_SIZE_AT + 4)?);
    let height = big_endian_u32(bytes.get(PNG_SIZE_AT + 4..PNG_SIZE_AT + 8)?);

    (width > 0 && height > 0).then_some((width, height))
}

fn big_endian_u32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0, |total, &byte| total << 8 | u32::from(byte))
}

/*
 * walks the jpeg's segments to the frame header (SOF0 to SOF15, apart from
 * DHT, JPG and DAC, which share the range), which holds the height, then the
 * width
 */
fn jpeg_size(bytes: &[u8]) -> Option<Size> {
    let mut at = JPEG.len();

    loop {
        // markers may be padded with extra 0xFF bytes
        while *bytes.get(at)? == JPEG_MARKER && *bytes.get(at + 1)? == JPEG_MARKER {
            at += 1;
        }

        if *bytes.get(at)? != JPEG_MARKER {
            return None;
        }

        let marker = *bytes.get(at + 1)?;

        // markers without a length: restarts, start of image, and the temporary one
        if (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
            at += 2;
            continue;
        }

        // the image data starts or ends before any frame header: nothing to find
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }

        let length = usize::from(*bytes.get(at + 2)?) << 8 | usize::from(*bytes.get(at + 3)?);

        let is_frame = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);

        if is_frame {
            let height = u32::from(*bytes.get(at + 5)?) << 8 | u32::from(*bytes.get(at + 6)?);
            let width = u32::from(*bytes.get(at + 7)?) << 8 | u32::from(*bytes.get(at + 8)?);

            return (width > 0 && height > 0).then_some((width, height));
        }

        at += 2 + length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Size = (3840, 2160);

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG.to_vec();

        bytes.extend([0, 0, 0, 13]);
        bytes.extend(b"IHDR");
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());

        bytes
    }

    // start of image, an APP1 segment of filler, a DHT, then the frame header
    fn jpeg_header(width: u16, height: u16, padding: usize) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];

        let app_length = (padding + 2) as u16;
        bytes.extend([0xFF, 0xE1]);
        bytes.extend(app_length.to_be_bytes());
        bytes.extend(vec![0; padding]);

        bytes.extend([0xFF, 0xC4, 0x00, 0x03, 0x00]);

        bytes.extend([0xFF, 0xFF, 0xC0, 0x00, 0x11, 0x08]);
        bytes.extend(height.to_be_bytes());
        bytes.extend(width.to_be_bytes());
        bytes.extend([0x03; 4]);

        bytes
    }

    fn source(width: u32, height: u32) -> Source {
        Source {
            width,
            height,
            stamp: 0x0a1b_2c3d_4e5f_6789,
        }
    }

    #[test]
    fn png_sizes_come_from_the_header() {
        assert_eq!(
            size_from_header(&png_header(6400, 3600)),
            Some((6400, 3600))
        );
        assert_eq!(size_from_header(&png_header(0, 3600)), None);
        assert_eq!(size_from_header(&png_header(6400, 3600)[..20]), None);
    }

    #[test]
    fn jpeg_sizes_come_from_the_frame_header() {
        assert_eq!(
            size_from_header(&jpeg_header(4000, 3000, 10)),
            Some((4000, 3000))
        );

        // a long metadata segment is skipped by its length
        assert_eq!(
            size_from_header(&jpeg_header(1920, 1080, 60_000)),
            Some((1920, 1080))
        );

        // cut off before the frame header
        let cut = jpeg_header(4000, 3000, 100);
        assert_eq!(size_from_header(&cut[..50]), None);

        // the image data starting first means there's no size to find
        assert_eq!(
            size_from_header(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]),
            None
        );
    }

    #[test]
    fn other_formats_have_no_size() {
        assert_eq!(size_from_header(b"GIF89a\x10\x00\x10\x00"), None);
        assert_eq!(size_from_header(&[]), None);
    }

    #[test]
    fn only_bigger_pictures_get_a_copy() {
        // rei.png and blossom.png are bigger than the screen both ways
        assert!(source(6400, 3600).needs_copy(SCREEN));
        assert!(source(4608, 2596).needs_copy(SCREEN));

        // girl.png, and pictures as big as the screen, are used as they are
        assert!(!source(1920, 1080).needs_copy(SCREEN));
        assert!(!source(3840, 2160).needs_copy(SCREEN));

        // a tall picture narrower than the screen isn't scaled up either
        assert!(!source(3000, 5000).needs_copy(SCREEN));

        assert!(source(1920, 1080).needs_copy(THUMBNAIL));
    }

    #[test]
    fn copies_are_named_by_picture_version_and_size() {
        let picture = Path::new("/home/someone/Pictures/Wallpapers/fuji.jpg");

        assert_eq!(
            source(6000, 4000).copy_name(picture, SCREEN),
            "fuji-0a1b2c3d4e5f6789-3840x2160.png"
        );
        assert_eq!(
            source(6000, 4000).copy_name(picture, THUMBNAIL),
            "fuji-0a1b2c3d4e5f6789-480x270.png"
        );
    }

    #[test]
    fn an_edited_picture_gets_a_new_stamp() {
        let folder = std::env::temp_dir().join(format!("pyrrha-copies-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();

        let picture = folder.join("edited.png");

        fs::write(&picture, png_header(4000, 3000)).unwrap();
        let before = Source::read(&picture).unwrap();

        let mut longer = png_header(4000, 3000);
        longer.push(0);
        fs::write(&picture, longer).unwrap();
        let after = Source::read(&picture).unwrap();

        assert_eq!((before.width, before.height), (4000, 3000));
        assert_ne!(before.stamp, after.stamp);

        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn small_pictures_are_ready_as_they_are() {
        let folder = Path::new("/nowhere");
        let picture = Path::new("/home/someone/small.png");

        assert_eq!(
            source(1920, 1080).existing(picture, SCREEN, folder),
            Some(picture.to_path_buf())
        );
        assert_eq!(source(6400, 3600).existing(picture, SCREEN, folder), None);
    }

    #[test]
    fn physical_sizes_use_the_scale() {
        let answer = r#"{"monitors":[
            {"name":"DP-1","width":2560,"height":1440,"scale":1.5},
            {"name":"HDMI-A-1","width":1920,"height":1080},
            {"name":"","width":800,"height":600,"scale":1}
        ]}"#;

        let sizes = physical_sizes(&json::parse(answer).unwrap());

        assert_eq!(sizes.get("DP-1"), Some(&(3840, 2160)));
        assert_eq!(sizes.get("HDMI-A-1"), Some(&(1920, 1080)));
        assert_eq!(sizes.len(), 2);
    }

    #[test]
    fn the_shown_background_goes_first_then_the_tiles() {
        let job = |name: &str, size: Size| Job {
            picture: PathBuf::from(name),
            size,
            output: PathBuf::from(format!("{name}-copy")),
        };

        let jobs = [
            job("a", SCREEN),
            job("b", SCREEN),
            job("a", THUMBNAIL),
            job("b", THUMBNAIL),
        ];

        assert_eq!(next_job(&jobs, Some(Path::new("b")), THUMBNAIL), 1);
        assert_eq!(next_job(&jobs, None, THUMBNAIL), 2);
        assert_eq!(next_job(&jobs, Some(Path::new("c")), THUMBNAIL), 2);
        assert_eq!(next_job(&jobs[..2], None, THUMBNAIL), 0);
    }

    #[test]
    fn only_unneeded_pngs_are_removed() {
        let files = [
            PathBuf::from("/cache/fuji-1-3840x2160.png"),
            PathBuf::from("/cache/fuji-2-3840x2160.png"),
            PathBuf::from("/cache/rei-3-480x270.part.png"),
            PathBuf::from("/cache/notes.txt"),
        ];

        let wanted = HashSet::from([PathBuf::from("/cache/fuji-2-3840x2160.png")]);

        assert_eq!(unwanted(&files, &wanted), [&files[0], &files[2]]);
    }

    #[test]
    fn the_tile_is_cropped_and_the_background_is_not() {
        assert!(filter(THUMBNAIL).ends_with(",crop=480:270"));
        assert!(filter(SCREEN).starts_with("scale=3840:2160:force_original_aspect_ratio=increase"));
        assert!(!filter(SCREEN).contains("crop"));
    }
}
