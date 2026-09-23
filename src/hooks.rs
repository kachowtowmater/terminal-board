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
//! Every run re-resolves the command and re-hashes the file. A hook whose file has changed,
//! moved or gone is not run.
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

/// Is tb running INSIDE a hook? Then no hook fires: a hook that calls `tb` would otherwise
/// call itself for ever. The hook is this machine's own trusted code, and it is the gate — so
/// the change it makes has already been through one.
pub fn in_hook() -> bool {
    crate::env("IN_HOOK").is_some()
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
        if !(1..=MAX_TIMEOUT_SECS).contains(&t) {
            return Err(BoardError(
                format!("a hook timeout is 1 to {MAX_TIMEOUT_SECS} seconds, not {t} — try 'tb trust {name} --timeout 10 -- COMMAND'"),
                Code::InvalidValue,
            ));
        }
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

/// Trust a recorded hook by echoing back the digest that was shown. The file is hashed again
/// here: a hash that was right a minute ago is not a hash that is right now.
pub fn confirm(name: &str, given: &str) -> Result<(PathBuf, String)> {
    let Some(e) = entry(name)? else {
        return Err(BoardError(
            format!("this machine does not know a hook called '{name}' — record it with 'tb trust {name} -- COMMAND'"),
            Code::InvalidValue,
        ));
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
    let Some(argv0) = e.argv.first() else {
        return State::Missing("no command is recorded".into());
    };
    let path = match resolve(argv0) {
        Ok(p) => p,
        Err(e) => return State::Missing(e.0),
    };
    if let Some(trusted) = &e.path {
        if Path::new(trusted) != path {
            return State::Missing(format!("'{argv0}' now resolves to {} — it was {trusted}", path.display()));
        }
    }
    let now = match sha256::of_file(&path) {
        Ok(d) => d,
        Err(err) => return State::Missing(format!("cannot read {}: {err}", path.display())),
    };
    match &e.sha256 {
        None => State::Untrusted,
        Some(was) if *was != now => State::Changed { now },
        Some(_) => match unsafe_writers(&path) {
            Some(why) => State::Unsafe(why),
            None => State::Trusted,
        },
    }
}

/// Anyone besides the owner who could rewrite what runs: the command itself, or the settings
/// that name it. (On a system without file modes there is nothing to check.)
fn unsafe_writers(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        for (what, p) in [("the command", path.to_path_buf()), ("the settings file", machine::path())] {
            if let Some(mode) = crate::fsperm::mode_of(&p) {
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
        let _ = path;
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
pub fn fire(event: Event, name: &str, payload: &str) -> Result<Run> {
    let refused = |how: String| BoardError(format!("hook refused — {how}"), Code::HookRefused);
    let glass = "or make the change with --break-glass \"why\" (recorded on the card)";
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
    match state(&e) {
        State::Trusted => {}
        State::Untrusted => {
            return Err(refused(format!(
                "the hook '{name}' is not trusted on this machine — run 'tb trust {name}' to see its file and digest, then 'tb trust {name} --sha256 HEX', {glass}"
            )))
        }
        State::Changed { now } => {
            return Err(refused(format!(
                "the hook '{name}' has changed since it was trusted — look at what it does now, then 'tb trust {name} --sha256 {now}', {glass}"
            )))
        }
        State::Missing(why) => {
            return Err(refused(format!("the hook '{name}' cannot be run: {why} — record it again with 'tb trust {name} -- COMMAND', {glass}")))
        }
        State::Unsafe(why) => {
            return Err(refused(format!("the hook '{name}' is not safe to run: {why} — fix the permissions, then run it again, {glass}")))
        }
    }
    let timeout = Duration::from_secs(e.timeout_secs.clamp(1, MAX_TIMEOUT_SECS));
    let started = Instant::now();
    let out = match run(&e.argv, payload, timeout, event, name) {
        Ok(o) => o,
        Err(err) => return Err(refused(format!("the hook '{name}' could not be started: {err} — check 'tb trust {name}', {glass}"))),
    };
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
pub fn fire_after(name: &str, payload: &str) -> std::result::Result<Run, String> {
    fire(Event::PostChange, name, payload).map_err(|e| e.0)
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
fn run(argv: &[String], payload: &str, timeout: Duration, event: Event, name: &str) -> std::io::Result<Output> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // a hook that runs `tb` must not fire hooks again, for ever
        .env("TB_IN_HOOK", "1")
        .env("TB_HOOK_EVENT", event.name())
        .env("TB_HOOK_NAME", name);
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
