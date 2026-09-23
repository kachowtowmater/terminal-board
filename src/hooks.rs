//! Hooks: the one seam by which commands on THIS MACHINE can see, and refuse, a change.
//!
//! # Why the command is not in the board file
//!
//! A board is a file people copy, mail and check into a repository. A command stored inside it
//! would be code that runs when someone else opens that file — the board would be a script
//! delivery system. So the board file holds at most a NAME (`hook`, `hook-after`), and the
//! command that name stands for lives in this machine's own settings
//! (`~/.config/terminal-board/config.json`, [`crate::machine`]), which travels with nobody.
//! A copied board can ASK for a gate; it can never say what the gate is.
//!
//! # Trust is given once, explicitly, without a prompt
//!
//! `tb trust NAME -- COMMAND …` records the command and prints the file it resolved to and
//! that file's SHA-256 — and the hook stays UNTRUSTED. It runs only after
//! `tb trust NAME --sha256 HEX` echoes back the hash the machine just showed: you cannot
//! trust what you were not shown, and no step needs a terminal (the client's users are agents).
//! Every run re-resolves the command, opens the file ONCE, hashes what it read, and runs that
//! same open file (a private copy of those same bytes where the platform cannot run an open
//! file) — never the path looked up again, so a file renamed over it in between never runs.
//! A hook whose file has changed, moved or gone is not run.
//!
//! # Fail closed
//!
//! A pre-change hook decides. Anything other than "exit 0 within the time limit" REFUSES the
//! change and writes nothing: a non-zero exit (the last line it printed becomes the hint), a
//! timeout, a command that cannot be run, a hook this machine does not know, an untrusted or
//! changed one, or settings that cannot be read. `--force` does NOT skip it (the proposal it
//! is handed says `"forced": true`). The way past a hook is `--break-glass "why"`, which is
//! recorded on the card and on the board — so a broken hook can never brick a board, and
//! never silently.
//!
//! # What it costs when it goes wrong
//!
//! The hook runs OUTSIDE the board's write transaction (`crate::store::Store::transition` asks
//! for it before opening one), so a hook that hangs holds no lock: every other agent keeps
//! working, and only the change that asked for it waits (up to its timeout, 10s by default).
//! After the hook answers, the change is applied under a fresh lock and refused if the card
//! moved meanwhile, so an answer about a card that no longer looks like what the hook saw is
//! never acted on.
//!
//! # One seam, not four
//!
//! [`Event`] names the moments. `pre-change` and `post-change` are here; the side panel and a
//! generic sync are the same mechanism with another event name, and will not need another
//! trust model, another config shape or another way to be turned off.

pub mod sha256;

use crate::machine;
use crate::store::{BoardError, Code, Result};
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The top-level settings key this module owns.
const KEY: &str = "hooks";
/// How long a hook may take before it is stopped and the change refused.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;
pub const MAX_TIMEOUT_SECS: u64 = 600;
/// How much of a hook's output is kept (the rest is read and dropped, so it never blocks).
const MAX_OUTPUT: usize = 64 * 1024;
/// How much of its last line becomes the hint.
const MAX_HINT: usize = 300;

/// A moment a hook can be attached to. The board names a hook per event; the settings say
/// what each name runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// Before a card changes column. Its answer decides.
    PreChange,
    /// After the change is written. Its answer is ignored (a failure is reported, not acted on).
    PostChange,
}

impl Event {
    /// The name in the JSON a hook is handed.
    pub fn name(self) -> &'static str {
        match self {
            Event::PreChange => "pre-change",
            Event::PostChange => "post-change",
        }
    }

    /// The board setting that names this event's hook.
    pub fn key(self) -> &'static str {
        match self {
            Event::PreChange => "hook",
            Event::PostChange => "hook-after",
        }
    }

    pub fn all() -> [Event; 2] {
        [Event::PreChange, Event::PostChange]
    }
}

/// What one run did, for the `hook` event on the card.
pub struct Run {
    pub name: String,
    pub event: Event,
    pub ms: u128,
    pub code: i32,
}

impl Run {
    /// `approve allowed the move in 12ms` — what `tb show` prints under the card.
    pub fn line(&self) -> String {
        format!("{} {} in {}ms", self.name, if self.code == 0 { "allowed" } else { "failed" }, self.ms)
    }
}

/// One trusted (or not yet trusted) command on this machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// The command and its arguments, exactly as `tb trust NAME -- …` was given them.
    pub argv: Vec<String>,
    /// The file `argv[0]` resolved to when trust was recorded.
    pub path: Option<String>,
    /// The digest that file had then. Present = trusted.
    pub sha256: Option<String>,
    pub timeout_secs: u64,
}

/// Whether a hook would run right now, and why not.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Trusted,
    /// Recorded, never confirmed with `--sha256`.
    Untrusted,
    /// The file is gone, or `argv[0]` now resolves somewhere else.
    Missing(String),
    /// The file is there but is not the one that was trusted.
    Changed { now: String },
    /// Anyone on this machine could rewrite the command (or these settings).
    Unsafe(String),
}

impl State {
    pub fn word(&self) -> &'static str {
        match self {
            State::Trusted => "trusted",
            State::Untrusted => "NOT TRUSTED",
            State::Missing(_) => "MISSING",
            State::Changed { .. } => "CHANGED",
            State::Unsafe(_) => "UNSAFE",
        }
    }
}

/// `[a-z0-9_-]`, 1 to 32 — a name that is safe to print, to store and to type.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

// ---------------------------------------------------------------- a hook's own `tb` calls
//
// A hook that itself runs `tb` against the board that asked for it must not fire that hook
// again (it would call itself for ever), and a plain environment variable cannot be what says
// so: anyone can set one. So each run gets a TICKET — a random token handed to the hook in
// `TB_HOOK_TOKEN`, and a private file (`hook-runs/TOKEN.run` beside this machine's settings,
// mode 0600 in a 0700 directory of this user's) that names the tb process running the hook
// and the board it runs for. The file is removed when the hook ends. A nested `tb` skips the
// hook only when its token names a ticket that exists, belongs to a tb process that is still
// alive, and is for the SAME board — and even then the change is recorded (`hook-nested`, on
// the card and in the board log), never silent. `TB_IN_HOOK=1` is still set for a hook to
// read, but it decides nothing.

/// The variable a hook's own `tb` calls find their run's token in.
const TOKEN_VAR: &str = "TB_HOOK_TOKEN";
/// How many hooks deep this process is (0 outside any hook). Forging it can only make tb
/// refuse sooner, never skip a hook.
const DEPTH_VAR: &str = "TB_HOOK_DEPTH";
/// A hook whose own `tb` call asks another board's hook, whose own call asks another… — cut
/// off (refused) at this depth instead of running until every timeout fires.
pub const MAX_DEPTH: u32 = 4;

/// The run a nested `tb` call was made from: the hook's name and event, as its ticket says.
#[derive(Clone, Debug, PartialEq)]
pub struct Nested {
    pub name: String,
    pub event: String,
}

/// Is this `tb` a hook's own call, made while that hook runs for the board `board` (its
/// canonical path)? Only a live ticket for that very board says yes; anything else — no
/// token, a token that is not one, a ticket that is gone, stale, someone else's, or for
/// another board — is `None`, and the hook runs as for anybody.
pub fn nested(board: &str) -> Option<Nested> {
    let token = std::env::var(TOKEN_VAR).ok()?;
    if board.is_empty() || !is_token(&token) {
        return None;
    }
    let dir = run_dir();
    let file = dir.join(format!("{token}.run"));
    if !private(&dir, true) || !private(&file, false) {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(&file).ok()?.take(64 * 1024).read_to_string(&mut text).ok()?;
    let mut parts = text.splitn(4, '\n');
    let pid: u32 = parts.next()?.trim().parse().ok()?;
    let name = parts.next()?.to_string();
    let event = parts.next()?.to_string();
    let for_board = parts.next()?.strip_suffix('\n')?;
    (for_board == board && alive(pid)).then_some(Nested { name, event })
}

fn is_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn depth() -> u32 {
    std::env::var(DEPTH_VAR).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

/// Where run tickets (and, where a file cannot be run from an open handle, the private copy
/// of a hook's command) live: beside this machine's settings, never beside a board.
fn run_dir() -> PathBuf {
    let settings = machine::path();
    let parent = settings.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    parent.join("hook-runs")
}

/// `path` is this user's own and nobody else's: a real directory (`dir`) or regular file, not
/// a link, owned by this user, no group/other permission bits. (No file modes: nothing to check.)
fn private(path: &Path, dir: bool) -> bool {
    let Ok(m) = std::fs::symlink_metadata(path) else { return false };
    if (dir && !m.is_dir()) || (!dir && !m.is_file()) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        m.uid() == sys::euid() && m.permissions().mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        sys::alive(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// The ticket for one run, removed (with the private copy of the command, if one was made)
/// when the run is over — whatever way it ends.
struct Ticket {
    token: String,
    /// Where a private copy of the command goes (unix only).
    #[cfg_attr(not(unix), allow(dead_code))]
    dir: PathBuf,
    file: PathBuf,
    copy: Option<PathBuf>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        if let Some(c) = &self.copy {
            let _ = std::fs::remove_file(c);
        }
    }
}

fn issue(board: &str, name: &str, event: Event) -> std::io::Result<Ticket> {
    let dir = run_dir();
    let mut mk = std::fs::DirBuilder::new();
    mk.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut mk, 0o700);
    mk.create(&dir)?;
    if !private(&dir, true) {
        return Err(std::io::Error::other(format!(
            "{} must be a directory only this user can use (chmod 700 it)",
            dir.display()
        )));
    }
    prune(&dir);
    let token = new_token()?;
    let file = dir.join(format!("{token}.run"));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let mut f = open.open(&file)?;
    let ticket = Ticket { token, dir, file, copy: None };
    write!(f, "{}\n{name}\n{}\n{board}\n", std::process::id(), event.name())?;
    Ok(ticket)
}

/// Tickets (and copies) left by a tb that was killed mid-run: harmless — their process is
/// gone, so `nested` never honours them — but not worth keeping.
fn prune(dir: &Path) {
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for f in list.flatten().take(1000) {
        let p = f.path();
        if p.extension().and_then(|e| e.to_str()) != Some("run") {
            continue;
        }
        let pid = std::fs::read_to_string(&p).ok().and_then(|t| t.lines().next().and_then(|l| l.trim().parse::<u32>().ok()));
        if !pid.is_some_and(alive) {
            let _ = std::fs::remove_file(&p);
            let _ = std::fs::remove_file(p.with_extension("cmd"));
        }
    }
}

/// 256 random bits, as 64 hex digits.
fn new_token() -> std::io::Result<String> {
    let mut b = [0u8; 32];
    #[cfg(unix)]
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    #[cfg(not(unix))]
    {
        use std::hash::{BuildHasher, Hasher};
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, chunk) in b.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(seed ^ i as u128);
            h.write_u32(std::process::id());
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    Ok(sha256::hex(&b))
}

/// The few C calls this module needs, without a `libc` dependency: every unix's C runtime has
/// them, and a Rust binary already links it.
#[cfg(unix)]
mod sys {
    extern "C" {
        fn geteuid() -> u32;
        fn kill(pid: i32, sig: i32) -> i32;
        #[cfg(target_os = "linux")]
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }

    pub fn euid() -> u32 {
        unsafe { geteuid() }
    }

    /// Signal 0 delivers nothing; it only asks whether `pid` exists (and is ours to signal).
    pub fn alive(pid: u32) -> bool {
        pid != 0 && i32::try_from(pid).is_ok_and(|p| unsafe { kill(p, 0) } == 0)
    }

    /// Clear close-on-exec on `fd`. Called in the child between fork and exec only (so the
    /// parent's own handle stays close-on-exec), where it has to be async-signal-safe: one
    /// `fcntl`, no allocation.
    #[cfg(target_os = "linux")]
    pub fn keep_across_exec(fd: i32) -> std::io::Result<()> {
        const F_SETFD: i32 = 2;
        if unsafe { fcntl(fd, F_SETFD, 0) } == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- the trust store

fn hooks_object() -> Result<Map<String, Value>> {
    let all = machine::load()?;
    match all.get(KEY) {
        None => Ok(Map::new()),
        Some(Value::Object(o)) => Ok(o.clone()),
        Some(_) => Err(BoardError(
            format!(
                "the settings file has a '{KEY}' that is not a set of hooks: {} — fix or remove that key",
                machine::path().display()
            ),
            Code::IoError,
        )),
    }
}

fn entry_of(v: &Value) -> Entry {
    let o = match v {
        Value::Object(o) => o.clone(),
        _ => Map::new(),
    };
    let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    Entry {
        argv: o
            .get("argv")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        path: text("path"),
        sha256: text("sha256"),
        timeout_secs: o.get("timeout_secs").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_SECS),
    }
}

/// Every hook this machine knows, by name.
pub fn entries() -> Result<Vec<(String, Entry)>> {
    Ok(hooks_object()?.iter().map(|(k, v)| (k.clone(), entry_of(v))).collect())
}

pub fn entry(name: &str) -> Result<Option<Entry>> {
    Ok(hooks_object()?.get(name).map(entry_of))
}

/// Change one hook's settings, keeping every other key in the file — and every field of this
/// hook that tb does not know about — exactly as it was.
fn edit(name: &str, change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    machine::update(|all| {
        let mut hooks = match all.get(KEY) {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        };
        let mut one = match hooks.get(name) {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        };
        change(&mut one);
        if one.is_empty() {
            hooks.remove(name);
        } else {
            hooks.insert(name.to_string(), Value::Object(one));
        }
        if hooks.is_empty() {
            all.remove(KEY);
        } else {
            all.insert(KEY.to_string(), Value::Object(hooks));
        }
    })
}

/// Record (or replace) what a name runs. The hook is left UNTRUSTED: the caller is given the
/// file and its digest to check, and `confirm` is a separate step.
pub fn record(name: &str, argv: &[String], timeout: Option<u64>) -> Result<(PathBuf, String)> {
    if !valid_name(name) {
        return Err(BoardError(
            format!("'{name}' is not a hook name (a-z 0-9 _ -, up to 32) — try 'tb trust approve -- /path/to/command'"),
            Code::InvalidValue,
        ));
    }
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(BoardError(
            format!("say what '{name}' runs — 'tb trust {name} -- /path/to/command --flag'"),
            Code::InvalidValue,
        ));
    }
    if let Some(t) = timeout {
        check_timeout(name, t)?;
    }
    let path = resolve(&argv[0])?;
    let digest = sha256::of_file(&path)
        .map_err(|e| BoardError(format!("cannot read {}: {e} — check the command's path", path.display()), Code::IoError))?;
    let argv_v: Vec<Value> = argv.iter().map(|a| json!(a)).collect();
    let t = timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let p = path.clone();
    edit(name, move |one| {
        one.insert("argv".into(), Value::Array(argv_v));
        one.insert("path".into(), json!(p.display().to_string()));
        one.insert("timeout_secs".into(), json!(t));
        // a command that changed is a command that has to be looked at again
        one.remove("sha256");
    })?;
    Ok((path, digest))
}

fn check_timeout(name: &str, t: u64) -> Result<()> {
    if (1..=MAX_TIMEOUT_SECS).contains(&t) {
        return Ok(());
    }
    Err(BoardError(
        format!("a hook timeout is 1 to {MAX_TIMEOUT_SECS} seconds, not {t} — try 'tb trust {name} --timeout 10'"),
        Code::InvalidValue,
    ))
}

/// The refusal for a name this machine has no hook under (`tb trust NAME …` on a name that
/// was never recorded, or was forgotten).
pub fn no_hook_err(name: &str) -> BoardError {
    BoardError(
        format!("this machine does not know a hook called '{name}' — record it with 'tb trust {name} -- COMMAND'"),
        Code::NoHook,
    )
}

/// `tb trust NAME --timeout SECS`: change how long a recorded hook may take. Nothing else
/// changes — the same command is trusted (or not) exactly as before, since a time limit is not
/// a different program. The refusal a timeout prints tells people to run exactly this.
pub fn set_timeout(name: &str, secs: u64) -> Result<()> {
    check_timeout(name, secs)?;
    if entry(name)?.is_none() {
        return Err(no_hook_err(name));
    }
    edit(name, move |one| {
        one.insert("timeout_secs".into(), json!(secs));
    })
}

/// Trust a recorded hook by echoing back the digest that was shown. The file is hashed again
/// here: a hash that was right a minute ago is not a hash that is right now.
pub fn confirm(name: &str, given: &str) -> Result<(PathBuf, String)> {
    let Some(e) = entry(name)? else {
        return Err(no_hook_err(name));
    };
    let given = given.trim().to_ascii_lowercase();
    if given.len() != 64 || !given.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(BoardError(
            format!("that is not a sha256 (64 hex characters) — run 'tb trust {name}' to see the one to paste"),
            Code::InvalidValue,
        ));
    }
    let path = resolve(e.argv.first().map(String::as_str).unwrap_or_default())?;
    let now = sha256::of_file(&path)
        .map_err(|e| BoardError(format!("cannot read {}: {e} — check the command's path", path.display()), Code::IoError))?;
    if now != given {
        return Err(BoardError(
            format!(
                "{} does not have that digest — it is now {now}; look at the file, then 'tb trust {name} --sha256 {now}'",
                path.display()
            ),
            Code::InvalidValue,
        ));
    }
    let p = path.display().to_string();
    let d = now.clone();
    edit(name, move |one| {
        one.insert("path".into(), json!(p));
        one.insert("sha256".into(), json!(d));
    })?;
    Ok((path, now))
}

/// Forget a hook. `Ok(false)`: this machine did not know it.
pub fn forget(name: &str) -> Result<bool> {
    if entry(name)?.is_none() {
        return Ok(false);
    }
    edit(name, |one| one.clear())?;
    Ok(true)
}

/// What `tb trust` reports for one recorded hook: is it ready to run, and why not.
pub fn state(e: &Entry) -> State {
    match check(e) {
        Ok(_) => State::Trusted,
        Err(s) => s,
    }
}

/// A hook's command, opened ONCE. Its digest was taken from this open file, and it is this
/// open file that runs — never the path looked up a second time, which a rename could have
/// pointed somewhere else in between (see [`command_for`]).
struct Pinned {
    /// The file `argv[0]` resolved to (what the hook is told as `TB_HOOK_PATH`).
    path: PathBuf,
    /// Held open so the file that runs is the file that was read (Linux runs it through
    /// this handle; elsewhere a copy of `bytes` runs instead).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    file: std::fs::File,
    /// Exactly the bytes that were hashed (what a private copy is written from, on unix).
    #[cfg_attr(not(unix), allow(dead_code))]
    bytes: Vec<u8>,
}

/// Resolve, open, hash and check a recorded hook: `Ok` only for one that is trusted, unchanged
/// and safe, with the very file that was checked held open to be run.
fn check(e: &Entry) -> std::result::Result<Pinned, State> {
    let Some(argv0) = e.argv.first() else {
        return Err(State::Missing("no command is recorded".into()));
    };
    let path = resolve(argv0).map_err(|e| State::Missing(e.0))?;
    if let Some(trusted) = &e.path {
        if Path::new(trusted) != path {
            return Err(State::Missing(format!("'{argv0}' now resolves to {} — it was {trusted}", path.display())));
        }
    }
    let cannot = |err: std::io::Error| State::Missing(format!("cannot read {}: {err}", path.display()));
    let mut file = std::fs::File::open(&path).map_err(cannot)?;
    let meta = file.metadata().map_err(cannot)?;
    if !meta.is_file() {
        return Err(State::Missing(format!("{} is not a file", path.display())));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(cannot)?;
    let now = sha256::of_bytes(&bytes);
    match &e.sha256 {
        None => Err(State::Untrusted),
        Some(was) if *was != now => Err(State::Changed { now }),
        Some(_) => match unsafe_writers(&meta, &path) {
            Some(why) => Err(State::Unsafe(why)),
            None => Ok(Pinned { path, file, bytes }),
        },
    }
}

/// Anyone besides the owner who could rewrite what runs: the command itself (the file that
/// was opened, by its own mode), or the settings that name it. (On a system without file
/// modes there is nothing to check.)
fn unsafe_writers(opened: &std::fs::Metadata, path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let settings = machine::path();
        let modes = [
            ("the command", path.to_path_buf(), Some(opened.permissions().mode() & 0o7777)),
            ("the settings file", settings.clone(), crate::fsperm::mode_of(&settings)),
        ];
        for (what, p, mode) in modes {
            if let Some(mode) = mode {
                if mode & 0o022 != 0 {
                    return Some(format!(
                        "{what} can be changed by other users: {} is mode {} — chmod go-w it",
                        p.display(),
                        crate::fsperm::fmt_mode(mode)
                    ));
                }
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = (opened, path);
        None
    }
}

/// `argv[0]` as an absolute file: a path as given (with links followed), a bare name looked up
/// in `PATH`. Never a shell — there is no shell anywhere in this module, on any platform, so a
/// hook's own `PATH`/`PATHEXT`/associations decide nothing tb does not decide first.
pub fn resolve(argv0: &str) -> Result<PathBuf> {
    let cannot = |why: &str| BoardError(format!("cannot use '{argv0}' as a command: {why} — give the path to an executable file"), Code::InvalidValue);
    if argv0.trim().is_empty() {
        return Err(cannot("it is empty"));
    }
    let direct = Path::new(argv0);
    let found = if direct.components().count() > 1 || direct.is_absolute() {
        Some(direct.to_path_buf())
    } else {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).map(|d| d.join(argv0)).find(|c| is_program(c)))
            .unwrap_or(None)
    };
    let Some(found) = found else {
        return Err(cannot("it is not in PATH"));
    };
    if !found.exists() {
        return Err(cannot("there is no such file"));
    }
    if !is_program(&found) {
        return Err(cannot("it is not an executable file"));
    }
    found.canonicalize().map_err(|e| cannot(&format!("{e}")))
}

fn is_program(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Where the settings that hold hooks live, for a message that has to say so.
pub fn path_hint() -> String {
    machine::path().display().to_string()
}

// ---------------------------------------------------------------- the proposal

/// What a hook is told, as the JSON object written to its standard input, one line:
/// `{"v":1,"event":"pre-change","board":"work","card":{…},"from":"doing","to":"review",
/// "actor":"bot-1","ts":1790000000,"forced":false,"reason":null}` — `card` is the very object
/// `tb show --json` prints, so a hook and a script read one shape.
#[allow(clippy::too_many_arguments)]
pub fn payload(event: Event, board: &str, card: &Value, from: &str, to: &str, actor: &str, ts: i64, forced: bool, reason: Option<&str>) -> String {
    let v = json!({
        "v": crate::contract::SCHEMA_VERSION,
        "event": event.name(),
        "board": board,
        "card": card,
        "from": from,
        "to": to,
        "actor": actor,
        "ts": ts,
        "forced": forced,
        "reason": reason,
    });
    format!("{v}\n")
}

// ---------------------------------------------------------------- running one

/// Run the hook `name` with `payload` on its standard input.
///
/// `Ok(Run)`: it answered 0 in time — the change may go on. `Err`: the change is refused, and
/// the error already says what happened and what to do about it — `error` is always the fixed
/// text `"hook refused"` (`Code::HookRefused`), so a caller can branch on `code` without
/// parsing prose; `hint` says why and what to do.
pub fn fire(event: Event, name: &str, payload: &str, board: &str) -> Result<Run> {
    let refused = |how: String| BoardError(format!("hook refused — {how}"), Code::HookRefused);
    let glass = "or make the change with --break-glass \"why\" (recorded on the card)";
    let deep = depth();
    if deep >= MAX_DEPTH {
        return Err(refused(format!(
            "the hook '{name}' was asked from inside {deep} other hook runs — a hook's own tb calls keep asking hooks; change one of them not to, {glass}"
        )));
    }
    let e = match entry(name) {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Err(refused(format!(
                "this machine does not know the hook '{name}', which this board asks for — say what it runs with 'tb trust {name} -- COMMAND', turn it off with 'tb config {} --off', {glass}",
                event.key()
            )))
        }
        // settings tb cannot read means a gate tb cannot apply: that is a refusal, not a pass
        Err(why) => {
            return Err(refused(format!(
                "the hook '{name}' cannot be checked: {} — fix the settings file, turn the hook off with 'tb config {} --off', {glass}",
                why.0,
                event.key()
            )))
        }
    };
    let pinned = match check(&e) {
        Ok(p) => p,
        Err(State::Trusted) => unreachable!("check() never reports Trusted as an error"),
        Err(State::Untrusted) => {
            return Err(refused(format!(
                "the hook '{name}' is not trusted on this machine — run 'tb trust {name}' to see its file and digest, then 'tb trust {name} --sha256 HEX', {glass}"
            )))
        }
        Err(State::Changed { now }) => {
            return Err(refused(format!(
                "the hook '{name}' has changed since it was trusted — look at what it does now, then 'tb trust {name} --sha256 {now}', {glass}"
            )))
        }
        Err(State::Missing(why)) => {
            return Err(refused(format!("the hook '{name}' cannot be run: {why} — record it again with 'tb trust {name} -- COMMAND', {glass}")))
        }
        Err(State::Unsafe(why)) => {
            return Err(refused(format!("the hook '{name}' is not safe to run: {why} — fix the permissions, then run it again, {glass}")))
        }
    };
    let timeout = Duration::from_secs(e.timeout_secs.clamp(1, MAX_TIMEOUT_SECS));
    let started = Instant::now();
    let not_started = |err: std::io::Error| refused(format!("the hook '{name}' could not be started: {err} — check 'tb trust {name}', {glass}"));
    // the ticket (and any private copy) lives until the run is over, then is removed
    let mut ticket = issue(board, name, event).map_err(not_started)?;
    let cmd = command_for(&pinned, &mut ticket).map_err(not_started)?;
    let out = run(cmd, &e.argv[1..], payload, timeout, event, name, &ticket.token, &pinned.path).map_err(not_started)?;
    drop(ticket);
    drop(pinned);
    let ms = started.elapsed().as_millis();
    if out.timed_out {
        return Err(refused(format!(
            "the hook '{name}' did not answer in {}s and was stopped — run it by hand to see where it waits, raise the limit with 'tb trust {name} --timeout SECS', {glass}",
            timeout.as_secs()
        )));
    }
    match out.code {
        Some(0) => Ok(Run { name: name.to_string(), event, ms, code: 0 }),
        code => {
            let said = last_line(&out.err, &out.out);
            let how = match (&said, code) {
                (Some(line), _) => line.clone(),
                (None, Some(c)) => format!("'{name}' exited {c} and printed nothing; run it by hand to see why, {glass}"),
                (None, None) => format!("'{name}' was stopped by a signal and printed nothing, {glass}"),
            };
            Err(refused(how))
        }
    }
}

/// Run `event`'s hook and, whatever happens, say what happened: post-change hooks do not
/// decide anything, so a failure is a line to report, never a change to undo.
pub fn fire_after(name: &str, payload: &str, board: &str) -> std::result::Result<Run, String> {
    fire(Event::PostChange, name, payload, board).map_err(|e| e.0)
}

/// How the checked file is started — always THAT file, never `argv[0]` looked up again:
///
/// - **Linux** (with `/proc`): the open handle itself, `/proc/self/fd/N`, kept open across
///   exec so a script's interpreter reads the same file too. What runs is the very file that
///   was hashed; renaming another file over the path at any moment changes nothing.
/// - **Other unix** (macOS, BSD) — or Linux without `/proc`: a private copy of exactly the
///   bytes that were hashed, written to this user's 0700 `hook-runs` directory and removed
///   after the run. A rename in the command's own directory cannot reach it.
/// - **Elsewhere** (Windows): the canonical path that was hashed. A swap between the hash and
///   the start is still possible there; this is the one platform where it is.
///
/// Either way `argv[0]` is the resolved path, and `TB_HOOK_PATH` names it — a script's own
/// `$0` is the handle or the copy, not where it lives.
fn command_for(p: &Pinned, ticket: &mut Ticket) -> std::io::Result<Command> {
    #[cfg(target_os = "linux")]
    {
        if Path::new("/proc/self/fd").is_dir() {
            return Ok(by_handle(p));
        }
    }
    #[cfg(unix)]
    {
        by_copy(p, ticket)
    }
    #[cfg(not(unix))]
    {
        let _ = ticket;
        Ok(Command::new(&p.path))
    }
}

/// Linux: run the open handle itself (`/proc/self/fd/N`), kept open across exec.
#[cfg(target_os = "linux")]
fn by_handle(p: &Pinned) -> Command {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt;
    let fd = p.file.as_raw_fd();
    let mut cmd = Command::new(format!("/proc/self/fd/{fd}"));
    cmd.arg0(&p.path);
    // SAFETY: runs in the forked child before exec; `keep_across_exec` is one fcntl call on
    // an fd number copied in — async-signal-safe, no allocation, no locks.
    unsafe {
        cmd.pre_exec(move || sys::keep_across_exec(fd));
    }
    cmd
}

/// Any unix: run a private copy (0700, in this user's 0700 `hook-runs`) of exactly the bytes
/// that were hashed; the ticket removes it when the run is over.
#[cfg(unix)]
fn by_copy(p: &Pinned, ticket: &mut Ticket) -> std::io::Result<Command> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    let copy = ticket.dir.join(format!("{}.cmd", ticket.token));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&copy)?;
    ticket.copy = Some(copy.clone());
    f.write_all(&p.bytes)?;
    drop(f);
    let mut cmd = Command::new(&copy);
    cmd.arg0(&p.path);
    Ok(cmd)
}

struct Output {
    code: Option<i32>,
    out: String,
    err: String,
    timed_out: bool,
}

/// Isolating a hook's process GROUP, so a timeout can take down everything it forked, not only
/// the one pid `spawn` returns (see `run`'s doc comment for the orphaned-grandchild problem
/// this exists to close). Unix only: on any other platform `isolate` is a no-op and `kill_all`
/// falls back to `Child::kill` (the direct child only) — this crate has nothing to say about
/// Windows Job Objects, the equivalent primitive there, until it has to.
mod pgroup {
    use std::process::{Child, Command};

    #[cfg(unix)]
    pub fn isolate(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        // 0: the kernel makes the child's own pid its process group id (`setpgid(0, 0)`
        // before exec) — every process IT forks inherits that group unless it changes its
        // own, which is exactly the group `kill_all` below signals as a whole.
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    pub fn isolate(_cmd: &mut Command) {}

    #[cfg(unix)]
    pub fn kill_all(child: &mut Child) {
        // No `libc` dependency for one syscall: `kill` is part of every unix's C runtime,
        // which a Rust binary already links, so this needs no new entry in Cargo.toml. A
        // NEGATIVE pid is POSIX for "every process in that group" — `isolate` above is what
        // makes the child's own pid equal to its group id, so this is that whole group.
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGKILL: i32 = 9;
        let pgid = child.id() as i32;
        // a killed group with nothing left in it is not an error worth surfacing here —
        // `child.wait()` right after this is what actually observes the outcome
        unsafe {
            kill(-pgid, SIGKILL);
        }
    }
    #[cfg(not(unix))]
    pub fn kill_all(child: &mut Child) {
        let _ = child.kill();
    }
}

/// The last thing the hook said, as the hint: the last non-empty line of its standard error,
/// else of its standard output. It is text from another program, so it is cut to a line
/// length here; every refusal text that carries it is sanitized the same way every other
/// stored or remote string tb prints is, at the one place tb renders output (`clean_json` /
/// `text::sanitize*`), so control bytes and escape sequences never reach a terminal or a log.
fn last_line(err: &str, out: &str) -> Option<String> {
    for text in [err, out] {
        if let Some(line) = text.lines().map(str::trim).rfind(|l| !l.is_empty()) {
            let cut: String = line.chars().take(MAX_HINT).collect();
            return Some(if cut.len() < line.len() { format!("{cut}…") } else { cut });
        }
    }
    None
}

/// Start the command, hand it the proposal, and wait — for at most `timeout`, after which it
/// is killed and reaped. No shell, ever: the recorded argv is passed as it is.
///
/// Three helpers do the talking, so nothing can deadlock: one writes the proposal (and stops
/// caring if the hook never reads it), one drains standard output, one standard error. All of
/// them end when the child does — including when it is killed.
///
/// On unix the child is started as the leader of its OWN process group (`process_group(0)`,
/// [`pgroup`]), and a timeout kills that whole GROUP, not just the one pid `spawn` returned. A
/// hook is a shell script more often than not, and `sh -c 'sleep 5'` (or any command that is
/// not the script's last line, so the shell cannot just exec into it) runs `sleep` as the
/// shell's own CHILD: killing only the shell orphans `sleep`, which keeps running and — this is
/// the part that actually bites — keeps holding the write end of the piped stdout/stderr this
/// function inherited, so `reading_out`/`reading_err` below never see end-of-file and this
/// function does not return until that orphan exits on its own. Measured: a 1s timeout on a
/// script whose last line is `sleep 5` took the full 5s without this. Killing the group takes
/// the orphan down with it, so the reader threads see EOF right away.
#[allow(clippy::too_many_arguments)]
fn run(mut cmd: Command, args: &[String], payload: &str, timeout: Duration, event: Event, name: &str, token: &str, path: &Path) -> std::io::Result<Output> {
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // for the hook to read; they decide nothing — the token is what lets the hook's own
        // `tb` calls on this board skip it (see `nested`), and only while this run is live
        .env("TB_IN_HOOK", "1")
        .env("TB_HOOK_EVENT", event.name())
        .env("TB_HOOK_NAME", name)
        .env("TB_HOOK_PATH", path)
        .env(TOKEN_VAR, token)
        .env(DEPTH_VAR, (depth() + 1).to_string());
    pgroup::isolate(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take();
    let text = payload.to_string();
    let writing = std::thread::spawn(move || {
        if let Some(mut w) = stdin.take() {
            // the hook is free to ignore its input; a closed pipe is not our problem
            let _ = w.write_all(text.as_bytes());
            let _ = w.flush();
        }
    });
    let (so, se) = (child.stdout.take(), child.stderr.take());
    let reading_out = std::thread::spawn(move || so.map(drain).unwrap_or_default());
    let reading_err = std::thread::spawn(move || se.map(drain).unwrap_or_default());
    let start = Instant::now();
    let mut wait = Duration::from_millis(1);
    let (status, timed_out) = loop {
        if let Some(s) = child.try_wait()? {
            break (Some(s), false);
        }
        if start.elapsed() >= timeout {
            pgroup::kill_all(&mut child);
            break (child.wait().ok(), true);
        }
        std::thread::sleep(wait.min(timeout.saturating_sub(start.elapsed())).max(Duration::from_millis(1)));
        wait = (wait * 2).min(Duration::from_millis(20));
    };
    // every helper ends now: the child (and, on unix, anything it forked) is gone, so every
    // copy of the pipes' write end is closed and the readers see EOF
    let _ = writing.join();
    let out = reading_out.join().unwrap_or_default();
    let err = reading_err.join().unwrap_or_default();
    Ok(Output { code: status.and_then(|s| s.code()), out, err, timed_out })
}

/// Read everything the hook writes — keeping the first [`MAX_OUTPUT`] bytes and dropping the
/// rest, so a hook that prints a gigabyte neither fills this process nor blocks on a full pipe.
fn drain(mut r: impl Read) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        if kept.len() < MAX_OUTPUT {
            let room = MAX_OUTPUT - kept.len();
            kept.extend_from_slice(&buf[..n.min(room)]);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_to_print_and_to_type() {
        assert!(valid_name("approve") && valid_name("a") && valid_name("law-1_x") && valid_name(&"a".repeat(32)));
        for bad in ["", "Approve", "a b", "../x", &"a".repeat(33), "hook!", "ø"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_hint_is_the_last_thing_it_said_and_is_bounded() {
        assert_eq!(last_line("", "").as_deref(), None);
        assert_eq!(last_line("   \n\n", "  ").as_deref(), None, "blank output says nothing");
        assert_eq!(last_line("a\nb\n", "out").as_deref(), Some("b"), "stderr wins");
        assert_eq!(last_line("", "only out\n").as_deref(), Some("only out"));
        assert_eq!(last_line("last\n\n\n", "").as_deref(), Some("last"), "trailing blank lines skipped");
        let long = last_line(&"x".repeat(1000), "").unwrap();
        assert_eq!(long.chars().count(), MAX_HINT + 1, "cut, with the cut marked");
        assert!(long.ends_with('…'));
    }

    #[test]
    fn the_proposal_is_one_json_line_in_the_documented_shape() {
        let card = json!({"id": 3, "title": "file the brief", "column": "doing"});
        let text = payload(Event::PreChange, "work", &card, "doing", "review", "bot-1", 1790000000, false, None);
        assert!(text.ends_with('\n') && text.lines().count() == 1, "one line: {text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["event"], "pre-change");
        assert_eq!(v["board"], "work");
        assert_eq!(v["card"]["id"], 3);
        assert_eq!((v["from"].as_str(), v["to"].as_str()), (Some("doing"), Some("review")));
        assert_eq!(v["actor"], "bot-1");
        assert_eq!(v["ts"], 1790000000);
        assert_eq!(v["forced"], false);
        assert!(v["reason"].is_null());
        let text = payload(Event::PostChange, "b", &card, "review", "doing", "rev", 1, true, Some("fix it"));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!((v["event"].as_str(), v["forced"].as_bool(), v["reason"].as_str()), (Some("post-change"), Some(true), Some("fix it")));
    }

    #[test]
    fn an_event_names_its_board_setting() {
        assert_eq!((Event::PreChange.name(), Event::PreChange.key()), ("pre-change", "hook"));
        assert_eq!((Event::PostChange.name(), Event::PostChange.key()), ("post-change", "hook-after"));
        assert_eq!(Event::all().len(), 2);
    }

    /// The file that runs is the file that was hashed, by every way this platform starts one
    /// — even when another file is renamed over its path after it was checked.
    #[cfg(unix)]
    #[test]
    fn the_file_that_runs_is_the_file_that_was_read_not_what_the_path_names_now() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let hook = dir.path().join("hook.sh");
        let put = |p: &Path, body: &str| {
            std::fs::write(p, body).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        type Start = fn(&Pinned, &mut Ticket) -> std::io::Result<Command>;
        #[cfg(target_os = "linux")]
        let handle: Option<(&str, Start)> = Some(("the open handle", |p, _| Ok(by_handle(p))));
        #[cfg(not(target_os = "linux"))]
        let handle: Option<(&str, Start)> = None;
        for (way, start) in std::iter::once(("a private copy", by_copy as Start)).chain(handle) {
            put(&hook, "#!/bin/sh\necho trusted\n");
            let mut file = std::fs::File::open(&hook).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            let pinned = Pinned { path: hook.clone(), file, bytes };
            // checked; now something else takes its name
            let other = dir.path().join("other.sh");
            put(&other, "#!/bin/sh\necho swapped\n");
            std::fs::rename(&other, &hook).unwrap();
            let token = "0".repeat(64);
            let mut ticket = Ticket { token: token.clone(), dir: dir.path().to_path_buf(), file: dir.path().join("none.run"), copy: None };
            let cmd = start(&pinned, &mut ticket).unwrap();
            let out = run(cmd, &[], "{}\n", Duration::from_secs(10), Event::PreChange, "h", &token, &hook).unwrap();
            assert_eq!((out.code, out.out.trim()), (Some(0), "trusted"), "{way}: {}", out.err);
            let copy = ticket.copy.clone();
            drop(ticket);
            assert!(copy.is_none_or(|c| !c.exists()), "{way}: the private copy is removed after the run");
        }
    }

    #[test]
    fn a_command_is_resolved_to_a_real_executable_file_or_refused() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hook.sh");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let e = resolve(script.to_str().unwrap()).unwrap_err().0;
        assert!(e.contains("not an executable file") && e.contains(" — "), "{e}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(resolve(script.to_str().unwrap()).unwrap(), script.canonicalize().unwrap());
        }
        assert!(resolve("").unwrap_err().0.contains("it is empty"));
        assert!(resolve(&dir.path().join("nope").display().to_string()).unwrap_err().0.contains("no such file"));
        let e = resolve("tb-definitely-not-a-real-command-xyz").unwrap_err().0;
        assert!(e.contains("not in PATH"), "{e}");
        assert!(resolve(dir.path().to_str().unwrap()).unwrap_err().0.contains("not an executable file"), "a directory is not a command");
    }
}
