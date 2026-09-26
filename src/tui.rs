//! Interactive board (ratatui). Pure `draw` over `App` so it can be tested with TestBackend.

use crate::github::{self, GhView};
use crate::herdr::{self, Agent, AgentsState};
use crate::plain::{card_head, event_line, fit, meta_parts};
use crate::store::{fmt_age, CardDetail, Card, Snapshot, Store, COLUMNS};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::{Arc, Mutex};

mod layouts;
pub use layouts::View;
use std::time::{Duration, Instant};

pub const NARROW: u16 = 100;
/// Card boxes narrower than this carry `gh#N` on the meta line instead of the title line.
pub const NARROW_CARD: usize = 30;

/// The named views, picked from the pane shape every frame (or pinned with `L` /
/// `tb config layout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// small box for focusing on ONE card: counts, the card big (checklist, last note), bars
    Focus,
    /// a third of the height (wide + short, e.g. 126x22): columns left, GITHUB over AGENTS right
    ThirdH,
    /// a third of the width (narrow + tall, e.g. 50x70): stacked sections, GITHUB, AGENTS
    ThirdV,
    /// half the height or more (wide, e.g. 126x41): four columns, GITHUB and AGENTS below
    HalfH,
    /// half the width (tall, e.g. 70x70): the columns as a 2x2 grid, GITHUB and AGENTS below
    HalfV,
}

/// View selection thresholds (terminal cells are ~2.2x taller than wide, so a pane is
/// "tall" when `cols < rows * 2.2`).
pub const VIEW_RULES: &[(&str, &str)] = &[
    ("focus", "cols < 40, or rows < 16, or short and < 80 cols, or tall and < 30 rows"),
    ("third-h", "wide (cols >= rows*2.2), rows < 30, cols >= 80"),
    ("third-v", "tall (cols < rows*2.2), cols < 48, rows >= 30"),
    ("half-h", "wide, rows >= 30"),
    ("half-v", "tall, cols >= 48 (two 24-cell columns), rows >= 30"),
];

/// Layout preference -> view; `auto` picks by size (see `VIEW_RULES`).
pub fn pick_shape(pref: &str, w: u16, h: u16) -> Shape {
    match pref {
        "focus" => return Shape::Focus,
        "third-h" => return Shape::ThirdH,
        "third-v" => return Shape::ThirdV,
        "half-h" => return Shape::HalfH,
        "half-v" => return Shape::HalfV,
        _ => {}
    }
    let tall = (w as f32) < h as f32 * 2.2;
    if w < 40 || h < 16 {
        Shape::Focus
    } else if tall {
        if h < 30 {
            Shape::Focus
        } else if w < 2 * layouts::MIN_COLUMN_WIDTH {
            Shape::ThirdV
        } else {
            Shape::HalfV
        }
    } else if h < 30 {
        if w >= 80 {
            Shape::ThirdH
        } else {
            Shape::Focus
        }
    } else {
        Shape::HalfH
    }
}
const REFRESH: Duration = Duration::from_secs(2);
const HERDR_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Add(String),
    /// Second step of `a`: the title is settled, now asking for an optional due date — the
    /// same second-prompt shape `n` (note) and the review send-back already use, so a card
    /// can get a date without leaving the board (card #104). `title` is carried through
    /// unwritten until the date is checked: nothing is created if it is refused.
    AddDue { title: String, buf: String },
    Note { id: i64, buf: String, from_popup: bool },
    /// Sending a REVIEW card back to DOING: the reason being typed.
    SendBack { id: i64, buf: String },
    Popup(i64),
    AddCheck { id: i64, buf: String },
    /// Repo picker (`R`): typed filter and selected row (row 0 = "none").
    Picker { filter: String, sel: usize },
    /// Board picker (`B`): the selected row — `App::boards` first, then `App::archived_boards`.
    Boards { sel: usize },
    /// Popup for a GitHub PR (`pr`=true) or issue.
    GhItem { pr: bool, number: i64 },
    /// Popup for an agent (index into the agents list).
    AgentInfo(usize),
    /// A y/n question on the footer line.
    Confirm { action: Confirm, prompt: String },
    /// The card edit form.
    Edit(EditForm),
    /// The `?` help overlay.
    Help,
}

/// The whole UI is one foreground colour on one background colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub fg: Color,
    pub bg: Color,
}

/// Accent colours are the terminal's NAMED ANSI colours, so the user's theme sets the shades.
/// Frames and text are plain fg; colour lives only on small markers and tags.
pub const RED: Color = Color::Red;
pub const GREEN: Color = Color::Green;

/// TODO's colour: Claude Code's "bypass permissions" red, one shade per theme.
/// The only Rgb colours in the UI (warnings stay ANSI Red).
pub const TODO_RED_DARK: Color = Color::Rgb(255, 107, 128);
pub const TODO_RED_LIGHT: Color = Color::Rgb(171, 43, 63);

/// Column colour (frame, header, dot, card boxes) for a theme.
pub fn column_colour_in(col: &str, theme: &str) -> Color {
    match col {
        "doing" => Color::Blue,
        "review" => Color::Yellow,
        "done" => Color::Green,
        _ if theme == "light" => TODO_RED_LIGHT,
        _ => TODO_RED_DARK,
    }
}

fn frame(thick: bool, colour: Option<Color>) -> Block<'static> {
    let b = Block::default()
        .borders(Borders::ALL)
        .border_type(if thick { BorderType::Thick } else { BorderType::Plain });
    match colour {
        Some(c) => b.border_style(Style::default().fg(c)),
        None => b,
    }
}

fn red() -> Style {
    Style::default().fg(RED)
}

/// `light` = black on white; anything else = `dark` (white on black).
pub fn palette(theme: &str) -> Palette {
    if theme == "light" {
        Palette { fg: Color::Black, bg: Color::White }
    } else {
        Palette { fg: Color::White, bg: Color::Black }
    }
}

/// One checklist item for the FOCUS view: (number, text, done).
pub type CheckRow = (i64, String, bool);

/// What a y/n confirmation does on `y`.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    Delete(i64),
    /// Move a GitHub card to done although its issue/PR is still open.
    ForceDone(i64),
    /// Approve a REVIEW card the actor authored (the forced, logged path).
    ApproveOwn(i64),
    /// Move someone else's DOING card to the column the key asked for (card, target): the
    /// forced, logged path.
    NotMine(i64, String),
    /// Delete (or archive) someone else's DOING card: the prompt named the holder, so `y` is
    /// the forced, logged path.
    DeleteHeld(i64),
    /// A queue-order or checklist change on someone else's DOING card: the prompt named the
    /// holder, so `y` is the forced, logged path — like `NotMine`, but for `check`/`prio`
    /// instead of a column move.
    NotMineWrite(i64, HeldWrite),
}

/// What a board picker key does, at once.
enum BoardAct {
    Archive,
    Restore,
    /// An archived board, for good.
    Delete,
    /// A live board: archived, then deleted for good.
    DeleteLive,
}

/// A row of the board picker.
enum Picked {
    Live(crate::boards::BoardRow),
    Archived(String),
    None,
}

/// A queue-order or checklist write the store applies no guard to itself (like `note`, `check`
/// and `prio` are open to everyone at the store layer — the CLI guards them in `main.rs` with
/// `holder_check`/`log_forced`; this is the TUI's copy of the same rule for its own keys).
#[derive(Debug, Clone, PartialEq)]
pub enum HeldWrite {
    /// `prio` — `how` is `up`, `down`, `top` or `bottom`.
    Reorder(String),
    /// Toggle checklist item `n`.
    Check(i64),
    /// Add a checklist item with this text.
    AddCheck(String),
    /// Remove checklist item `n`.
    RemoveCheck(i64),
}

impl HeldWrite {
    /// Finishes "held by OWNER — … anyway?" — the same tokens `main.rs` uses for `--force`.
    fn what(&self) -> &'static str {
        match self {
            HeldWrite::Reorder(_) => "reorder it",
            HeldWrite::Check(_) => "tick it",
            HeldWrite::AddCheck(_) => "add to it",
            HeldWrite::RemoveCheck(_) => "remove it",
        }
    }

    /// The `force` event's verb — matches `main.rs`'s `did` for the same change.
    fn did(&self) -> &'static str {
        match self {
            HeldWrite::Reorder(_) => "reordered",
            HeldWrite::Check(_) => "checked",
            HeldWrite::AddCheck(_) => "added a check to",
            HeldWrite::RemoveCheck(_) => "removed a check from",
        }
    }
}

/// Title + description edit form (`e`). `cursor` is a char index into the active field.
#[derive(Debug, Clone, PartialEq)]
pub struct EditForm {
    pub id: i64,
    pub title: String,
    /// The due date as typed, `YYYY-MM-DD`; empty = no date (what `--due none` does).
    pub due: String,
    pub desc: String,
    /// What the fields read when the form opened (the save-conflict baseline).
    pub open_title: String,
    pub open_due: String,
    pub open_desc: String,
    /// 0 = title, 1 = due, 2 = description
    pub field: u8,
    pub cursor: usize,
    pub from_popup: bool,
}

/// How many fields the form has, in tab order.
pub const FORM_FIELDS: u8 = 3;

impl EditForm {
    fn text(&mut self) -> &mut String {
        match self.field {
            0 => &mut self.title,
            1 => &mut self.due,
            _ => &mut self.desc,
        }
    }

    /// The field's text, for the cursor and the width maths.
    pub fn field_text(&self) -> &str {
        match self.field {
            0 => &self.title,
            1 => &self.due,
            _ => &self.desc,
        }
    }

    /// Apply one editing key; returns false if the key isn't an edit key.
    pub fn key(&mut self, code: KeyCode) -> bool {
        let len = self.field_text().chars().count();
        let cur = self.cursor.min(len);
        match code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.field = match code {
                    KeyCode::BackTab => (self.field + FORM_FIELDS - 1) % FORM_FIELDS,
                    _ => (self.field + 1) % FORM_FIELDS,
                };
                self.cursor = self.field_text().chars().count();
            }
            KeyCode::Left => self.cursor = cur.saturating_sub(1),
            KeyCode::Right => self.cursor = (cur + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace if cur > 0 => {
                let t = self.text();
                let at = t.char_indices().nth(cur - 1).map(|(i, _)| i).unwrap_or(0);
                t.remove(at);
                self.cursor = cur - 1;
            }
            KeyCode::Delete if cur < len => {
                let t = self.text();
                let at = t.char_indices().nth(cur).map(|(i, _)| i).unwrap_or(t.len());
                t.remove(at);
            }
            KeyCode::Char(c) => {
                let t = self.text();
                let at = t.char_indices().nth(cur).map(|(i, _)| i).unwrap_or(t.len());
                t.insert(at, c);
                self.cursor = cur + 1;
            }
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }
}

/// Which area has keyboard focus: the columns, or one of the panels below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Columns,
    Github,
    Agents,
}

/// Where the background repo load drops its result.
pub type RepoSlot = Arc<Mutex<Option<std::result::Result<Vec<github::RepoEntry>, String>>>>;

#[derive(Debug, Clone, PartialEq)]
pub enum RepoState {
    Idle,
    Loading,
    Loaded(Vec<github::RepoEntry>),
    Error(String),
}

pub struct App {
    pub snap: Snapshot,
    pub agents: AgentsState,
    pub col: usize,
    pub row: [usize; 4],
    pub show_agents: bool,
    /// GitHub panel data (repo None = panel off) and its `G` toggle.
    pub gh: GhView,
    pub show_github: bool,
    pub mode: Mode,
    /// Scroll offset of the `?` help overlay (up/down, PgUp/PgDn in Mode::Help).
    pub help_scroll: u16,
    /// The largest useful `help_scroll` at the last render (the key handler clamps to it).
    pub help_max: std::cell::Cell<u16>,
    pub popup: Option<CardDetail>,
    pub status: Option<(String, bool)>,
    pub actor: String,
    /// Checklist cursor in the card popup.
    pub cursor: usize,
    /// Shape of the last frame drawn (sidebar navigation depends on it).
    pub last_shape: std::cell::Cell<Shape>,
    /// When a transient status (e.g. the layout name) should disappear.
    pub status_until: Option<Instant>,
    /// Repo picker data, loaded in the background.
    pub repos: RepoState,
    pub repos_rx: Option<RepoSlot>,
    /// Inline picker error (e.g. a typed repo that does not exist).
    pub picker_msg: Option<String>,
    /// Board picker rows (`tb boards`), re-read from disk each time `B` opens the overlay.
    pub boards: Vec<crate::boards::BoardRow>,
    /// Board picker: archived boards, one row per name (its newest archive), listed after
    /// the live ones.
    pub archived_boards: Vec<crate::boards::ArchiveRow>,
    /// Keyboard focus and the selected row inside each panel.
    pub focus: Focus,
    pub gh_sel: usize,
    pub ag_sel: usize,
    /// Panels actually drawn in the last frame: (github, agents).
    pub shown: std::cell::Cell<(bool, bool)>,
    /// Panels drawn as 1-line bars in the last frame: (github, agents).
    pub bars: std::cell::Cell<(bool, bool)>,
    /// Where things were drawn last frame, for spatial arrow keys: the four columns, and the
    /// GITHUB / AGENTS areas (panel or bar).
    pub col_rects: std::cell::Cell<[Rect; 4]>,
    pub area_rects: std::cell::Cell<[Option<Rect>; 2]>,
    /// FOCUS view: false until the user moves; until then it shows the default focus card.
    pub focus_nav: bool,
    /// Checklists of cards (id -> items), refreshed on reload, for the FOCUS view.
    pub focus_items: std::cell::RefCell<std::collections::HashMap<i64, Vec<CheckRow>>>,
    /// Board, or one panel full screen (Tab paging).
    pub view: View,
    /// Pre-select the current repo once the picker's list arrives.
    pub picker_preselect: bool,
    /// Card style each column was drawn with last frame: (column, "full" | "dense" |
    /// "compact"). Tests use it to prove one style per render.
    pub drawn_styles: std::cell::RefCell<Vec<(usize, &'static str)>>,
}

impl App {
    pub fn new(snap: Snapshot, actor: &str) -> App {
        let (show_agents, show_github) = (!snap.agents_panel_hidden, !snap.github_panel_hidden);
        App {
            snap,
            agents: AgentsState::Pending,
            col: 0,
            row: [0; 4],
            show_agents,
            gh: GhView::default(),
            show_github,
            mode: Mode::Normal,
            help_scroll: 0,
            help_max: std::cell::Cell::new(0),
            popup: None,
            status: None,
            actor: actor.to_string(),
            cursor: 0,
            last_shape: std::cell::Cell::new(Shape::HalfH),
            status_until: None,
            repos: RepoState::Idle,
            repos_rx: None,
            picker_msg: None,
            boards: Vec::new(),
            archived_boards: Vec::new(),
            focus: Focus::Columns,
            gh_sel: 0,
            ag_sel: 0,
            shown: std::cell::Cell::new((false, false)),
            bars: std::cell::Cell::new((false, false)),
            col_rects: std::cell::Cell::new([Rect::default(); 4]),
            area_rects: std::cell::Cell::new([None, None]),
            focus_nav: false,
            focus_items: Default::default(),
            view: View::Board,
            picker_preselect: false,
            drawn_styles: Default::default(),
        }
    }

    fn col_cards(&self, col: usize) -> Vec<&Card> {
        self.snap.on_board(COLUMNS[col])
    }

    pub fn selected(&self) -> Option<&Card> {
        let cards = self.col_cards(self.col);
        cards.get(self.row[self.col].min(cards.len().saturating_sub(1))).copied()
    }

    fn clamp(&mut self) {
        for c in 0..4 {
            let n = self.col_cards(c).len();
            self.row[c] = self.row[c].min(n.saturating_sub(1));
        }
    }

    pub fn focus_card(&mut self, id: i64) {
        for c in 0..4 {
            if let Some(i) = self.col_cards(c).iter().position(|k| k.id == id) {
                self.col = c;
                self.row[c] = i;
            }
        }
    }

    pub fn reload(&mut self, store: &Store) {
        match store.snapshot() {
            Ok(s) => {
                self.show_github = !s.github_panel_hidden;
                self.show_agents = !s.agents_panel_hidden;
                self.snap = s
            }
            Err(e) => self.status = Some((e.to_string(), true)),
        }
        if let Ok(v) = store.github_view() {
            self.gh = v;
        }
        // FOCUS view: keep the checklists of the cards it can show
        let mut items = std::collections::HashMap::new();
        for c in self.snap.cards.iter().filter(|c| c.column != "done") {
            if let Ok(d) = store.show(c.id) {
                if !d.checklist.is_empty() {
                    items.insert(c.id, d.checklist.iter().map(|i| (i.idx, i.text.clone(), i.done)).collect());
                }
            }
        }
        *self.focus_items.borrow_mut() = items;
        self.clamp();
        if let Some(d) = &self.popup {
            self.popup = store.show(d.card.id).ok();
        }
        // warnings raised opening (or, mid-session, re-opening) THIS board — a wide board
        // file, a backup made before a schema upgrade — are shown here rather than only on
        // stderr after the board exits, since the alternate screen hides stderr while it
        // runs (card #83). Keyed to `store.notice_key()` — the one function that computes
        // this (see its doc comment on `store::conn_notice_key`), the same one every
        // `notice::push_for` about this connection already used — so a keyed drain only ever
        // takes entries raised for this exact board, whatever shape its path was given in
        // (relative, through a symlink, `..` in it, `TB_DB` passing one through unchanged):
        // another board open in the same process (or, in the test binary, an unrelated test
        // running at the same time against its own tempdir) can never leak into this status
        // line. Only when the line is free: an unread one is never silently replaced by a
        // later one, and anything still queued when the board exits is left for `main`'s own
        // stderr print to catch, so a warning is never lost outright.
        if self.status.is_none() {
            let pending = store.notice_key().map(|k| crate::notice::take_unprinted_for(&k)).unwrap_or_default();
            if !pending.is_empty() {
                self.status = Some((pending.join(" · "), true));
            }
        }
    }

    fn report<T>(&mut self, r: crate::store::Result<T>, ok: impl FnOnce(&T) -> String) -> Option<T> {
        match r {
            Ok(v) => {
                self.status = Some((ok(&v), false));
                Some(v)
            }
            Err(e) => {
                self.status = Some((e.to_string(), true));
                None
            }
        }
    }

    /// Handle one key. Returns true to quit.
    pub fn handle_key(&mut self, key: KeyEvent, store: &mut Store) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        let actor = self.actor.clone();
        match self.mode.clone() {
            Mode::Add(mut buf) => match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter => {
                    if buf.trim().is_empty() {
                        self.mode = Mode::Normal;
                    } else {
                        // nothing is written yet — the card is only created once the due
                        // date (if any) has been checked, in `Mode::AddDue` below
                        self.mode = Mode::AddDue { title: buf, buf: String::new() };
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.mode = Mode::Add(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.mode = Mode::Add(buf);
                }
                _ => {}
            },
            // the due date, checked before anything is written and in the same words
            // `tb add --due` / the `e` form use, so all three refuse alike (card #104)
            Mode::AddDue { title, mut buf } => match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter => {
                    let typed = buf.trim();
                    let raw = if typed.is_empty() { crate::store::due::NONE.to_string() } else { typed.to_string() };
                    match crate::store::due::DueDate::parse(&raw, "tb add \"tag: title\" --due 2026-10-09") {
                        Ok(date) => {
                            self.mode = Mode::Normal;
                            let r = store.add(&title, "", &[], &actor);
                            if let Some(id) = self.report(r, |id| match &date {
                                Some(d) => format!("added #{id}, due {}", d.as_str()),
                                None => format!("added #{id}"),
                            }) {
                                if let Some(d) = &date {
                                    let dr = store.set_due(id, Some(d), &actor).map(|_| ());
                                    self.report(dr, |()| format!("added #{id}, due {}", d.as_str()));
                                }
                                self.reload(store);
                                self.focus_card(id);
                            }
                        }
                        Err(e) => {
                            // refused, exactly as `tb add --due` would: nothing is created,
                            // the typed date stays on screen to fix
                            self.status = Some((e.to_string(), true));
                            self.mode = Mode::AddDue { title, buf };
                        }
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.mode = Mode::AddDue { title, buf };
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.mode = Mode::AddDue { title, buf };
                }
                _ => {}
            },
            Mode::Note { id, mut buf, from_popup } => match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    if key.code == KeyCode::Enter && !buf.trim().is_empty() {
                        let r = store.note(id, &buf, &actor);
                        self.report(r, |_| format!("noted #{id}"));
                        self.reload(store);
                    }
                    self.mode = if from_popup { Mode::Popup(id) } else { Mode::Normal };
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.mode = Mode::Note { id, buf, from_popup };
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.mode = Mode::Note { id, buf, from_popup };
                }
                _ => {}
            },
            Mode::SendBack { id, mut buf } => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.status = Some(("cancelled — the card stays in review".into(), false));
                }
                KeyCode::Enter => {
                    if buf.trim().is_empty() {
                        self.mode = Mode::SendBack { id, buf };
                        return false;
                    }
                    self.mode = Mode::Normal;
                    let r = store.send_back(id, &buf, &actor);
                    if self.report(r, |c| format!("#{} sent back to {}", c.id, c.owner.as_deref().unwrap_or("doing"))).is_some() {
                        self.reload(store);
                        self.focus_card(id);
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.mode = Mode::SendBack { id, buf };
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.mode = Mode::SendBack { id, buf };
                }
                _ => {}
            },
            Mode::AddCheck { id, mut buf } => match key.code {
                KeyCode::Esc => self.mode = Mode::Popup(id),
                KeyCode::Enter => {
                    if !buf.trim().is_empty() {
                        if self.guard_write(id, HeldWrite::AddCheck(buf.clone())) {
                            return false;
                        }
                        self.commit_add_check(id, &buf, store);
                    }
                    self.mode = Mode::Popup(id);
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.mode = Mode::AddCheck { id, buf };
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.mode = Mode::AddCheck { id, buf };
                }
                _ => {}
            },
            Mode::Popup(id) => {
                let items = self.popup.as_ref().map(|d| d.checklist.clone()).unwrap_or_default();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.mode = Mode::Normal;
                        self.popup = None;
                    }
                    KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
                    KeyCode::Down => {
                        self.cursor = (self.cursor + 1).min(items.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(item) = items.get(self.cursor.min(items.len().saturating_sub(1))) {
                            let n = item.idx;
                            if self.guard_write(id, HeldWrite::Check(n)) {
                                return false;
                            }
                            self.commit_check(id, n, store);
                        }
                    }
                    KeyCode::Char('n') => {
                        self.mode = Mode::Note { id, buf: String::new(), from_popup: true }
                    }
                    KeyCode::Char('e') => self.open_edit(id, true),
                    // inside the popup, a/d act on the checklist only, never the board
                    KeyCode::Char('a') => self.mode = Mode::AddCheck { id, buf: String::new() },
                    KeyCode::Char('d') => {
                        if let Some(item) = items.get(self.cursor.min(items.len().saturating_sub(1))) {
                            let n = item.idx;
                            if self.guard_write(id, HeldWrite::RemoveCheck(n)) {
                                return false;
                            }
                            self.commit_remove_check(id, n, store);
                        }
                    }
                    _ => {}
                }
            }
            Mode::Picker { mut filter, sel } => {
                self.poll_repos();
                let rows = self.picker_rows(&filter).len();
                let fallback = rows == 0 && github::valid_repo(filter.trim());
                let n = rows.max(usize::from(fallback)) + 1; // + the "off" row
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.picker_msg = None;
                    }
                    KeyCode::Up => self.mode = Mode::Picker { filter, sel: sel.saturating_sub(1) },
                    KeyCode::Down => self.mode = Mode::Picker { filter, sel: (sel + 1).min(n - 1) },
                    KeyCode::Backspace => {
                        filter.pop();
                        self.picker_msg = None;
                        self.mode = Mode::Picker { filter, sel: 0 };
                    }
                    KeyCode::Char(c) => {
                        filter.push(c);
                        self.picker_msg = None;
                        let empty = self.picker_rows(&filter).is_empty();
                        let first = if !empty || github::valid_repo(filter.trim()) { 1 } else { 0 };
                        self.mode = Mode::Picker { filter, sel: first };
                    }
                    KeyCode::Enter => self.pick_repo(&filter, sel, store),
                    _ => {}
                }
            }
            Mode::Boards { sel } => {
                let last = (self.boards.len() + self.archived_boards.len()).saturating_sub(1);
                match key.code {
                    // esc leaves everything exactly as it was
                    KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
                    KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Boards { sel: sel.saturating_sub(1) },
                    KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Boards { sel: (sel + 1).min(last) },
                    KeyCode::Home => self.mode = Mode::Boards { sel: 0 },
                    KeyCode::End => self.mode = Mode::Boards { sel: last },
                    KeyCode::Enter => match self.picked(sel.min(last)) {
                        Picked::Live(row) => self.switch_board(&row, store),
                        Picked::Archived(name) => {
                            self.status = Some((format!("'{name}' is archived — r restores it, d deletes it for good"), false))
                        }
                        Picked::None => {}
                    },
                    KeyCode::Char(c @ ('a' | 'r' | 'd')) => self.board_key(c, sel.min(last)),
                    KeyCode::Char('*') => self.make_default(sel.min(last)),
                    _ => {}
                }
            }
            Mode::GhItem { pr, number } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
                KeyCode::Char('a') => self.add_gh_card(pr, number, store),
                KeyCode::Char('o') => self.open_in_browser(pr, number),
                _ => {}
            },
            Mode::AgentInfo(i) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
                KeyCode::Enter => {
                    if let Some(id) = self.agent_card(i).map(|c| c.id) {
                        self.mode = Mode::Normal;
                        self.focus = Focus::Columns;
                        self.focus_card(id);
                    }
                }
                _ => {}
            },
            Mode::Help => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                        self.mode = Mode::Normal;
                        self.help_scroll = 0;
                    }
                    KeyCode::Down => self.help_scroll = (self.help_scroll + 1).min(self.help_max.get()),
                    KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                    KeyCode::PageDown => self.help_scroll = (self.help_scroll + 10).min(self.help_max.get()),
                    KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                    KeyCode::Home => self.help_scroll = 0,
                    KeyCode::End => self.help_scroll = self.help_max.get(),
                    _ => {}
                }
            }
            Mode::Confirm { action, .. } => {
                self.mode = Mode::Normal;
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    match action {
                        // the holder rule: only a prompt that named the holder forces it
                        Confirm::Delete(id) => self.remove_confirmed(id, false, store),
                        Confirm::DeleteHeld(id) => self.remove_confirmed(id, true, store),
                        Confirm::ForceDone(id) => {
                            if self.ask_own_close(id, store) {
                                return false;
                            }
                            let r = store.move_to(id, "done", &actor);
                            if self.report(r, |c| format!("#{} -> done", c.id)).is_some() {
                                self.reload(store);
                                self.focus_card(id);
                            }
                        }
                        Confirm::NotMine(id, to) => {
                            let r = store.move_to_forced(id, &to, &actor);
                            if self.report(r, |c| format!("#{} -> {} (not yours, logged)", c.id, c.column)).is_some() {
                                self.reload(store);
                                self.focus_card(id);
                            }
                        }
                        Confirm::ApproveOwn(id) => {
                            // asked again at the moment of the force: the prompt named what it
                            // skips, and the verifier rule is never one of them
                            if self.verifier_refuses(id, store) {
                                return false;
                            }
                            let r = store.move_to_forced(id, "done", &actor);
                            if self.report(r, |c| format!("#{} -> done (own work, logged)", c.id)).is_some() {
                                self.reload(store);
                                self.focus_card(id);
                            }
                        }
                        Confirm::NotMineWrite(id, write) => self.commit_forced_write(id, write, store),
                    }
                } else {
                    self.status = Some(("cancelled".into(), false));
                }
            }
            Mode::Edit(mut form) => match key.code {
                KeyCode::Esc => {
                    self.mode = if form.from_popup { Mode::Popup(form.id) } else { Mode::Normal };
                }
                KeyCode::Enter => {
                    let typed = form.due.trim().to_string();
                    let due_changed = typed != form.open_due.trim();
                    // the date is checked before anything is written, in the words the CLI
                    // uses (store::due), so the form and `tb edit --due` refuse alike
                    let date = if due_changed {
                        let raw = if typed.is_empty() { "none".to_string() } else { typed.clone() };
                        match crate::store::due::DueDate::parse(&raw, &format!("tb edit {} --due 2026-10-09", form.id)) {
                            Ok(d) => Some(d),
                            Err(e) => {
                                self.status = Some((e.to_string(), true));
                                self.mode = Mode::Edit(form);
                                return false;
                            }
                        }
                    } else {
                        None
                    };
                    // the same stale-form rule the title and description follow: a date the
                    // person changed that someone else changed too is refused, not overwritten
                    if due_changed {
                        let now = store.card(form.id).ok().and_then(|c| c.due).unwrap_or_default();
                        if now.trim() != form.open_due.trim() {
                            self.status = Some((
                                format!("#{} changed while you were editing — the due date has a newer value; reopen with e", form.id),
                                true,
                            ));
                            self.mode = Mode::Edit(form);
                            return false;
                        }
                    }
                    // only the fields the person changed are written; a field they left at
                    // its open-time value but that moved on since is refused, not overwritten
                    let text_changed = form.title != form.open_title || form.desc != form.open_desc;
                    // a form nobody changed answers exactly as it did before this field
                    // existed: `nothing to change`, from the same call the CLI makes
                    let r = if text_changed || !due_changed {
                        store.edit(
                            form.id,
                            (form.title != form.open_title).then_some(form.title.as_str()),
                            (form.desc != form.open_desc).then_some(form.desc.as_str()),
                            &actor,
                            Some((form.open_title.as_str(), form.open_desc.as_str())),
                        )
                    } else {
                        store.card(form.id)
                    };
                    let saved = self.report(r, |c| format!("#{} saved", c.id)).is_some();
                    if saved && due_changed {
                        let r = store.set_due(form.id, date.as_ref().and_then(Option::as_ref), &actor).map(|_| ());
                        if self.report(r, |()| format!("#{} saved", form.id)).is_none() {
                            self.reload(store);
                            self.mode = Mode::Edit(form);
                            return false;
                        }
                    }
                    if saved {
                        self.reload(store);
                        self.focus_card(form.id);
                        self.mode = if form.from_popup { Mode::Popup(form.id) } else { Mode::Normal };
                    }
                }
                code => {
                    form.key(code);
                    self.mode = Mode::Edit(form);
                }
            },
            Mode::Normal if self.focus != Focus::Columns => return self.panel_key(key, store),
            Mode::Normal => return self.normal_key(key, store),
        }
        false
    }

    /// Start loading the repo list in the background (the picker shows `loading...`).
    pub fn start_repo_load(&mut self) {
        let slot = Arc::new(Mutex::new(None));
        let out = Arc::clone(&slot);
        std::thread::spawn(move || {
            let r = github::list_repos();
            if let Ok(mut g) = out.lock() {
                *g = Some(r);
            }
        });
        self.repos = RepoState::Loading;
        self.repos_rx = Some(slot);
    }

    /// Pick up a finished background repo load, if any.
    pub fn poll_repos(&mut self) {
        let done = self.repos_rx.as_ref().and_then(|rx| rx.lock().ok().and_then(|mut g| g.take()));
        if let Some(r) = done {
            self.repos = match r {
                Ok(v) => RepoState::Loaded(v),
                Err(e) => RepoState::Error(e),
            };
            self.repos_rx = None;
            if self.picker_preselect {
                self.picker_preselect = false;
                if let (Mode::Picker { filter, .. }, Some(cur)) = (&self.mode, self.gh.repo.clone()) {
                    let filter = filter.clone();
                    if let Some(i) = self.picker_rows(&filter).iter().position(|r| r.name_with_owner == cur) {
                        self.mode = Mode::Picker { filter, sel: i + 1 };
                    }
                }
            }
        }
    }

    /// Repos matching the typed filter (fuzzy on owner/name).
    /// Picker groups (owner, repos) matching the filter (case-insensitive substring on
    /// owner/name); empty groups are dropped.
    pub fn picker_groups(&self, filter: &str) -> Vec<(String, Vec<&github::RepoEntry>)> {
        let f = filter.trim().to_ascii_lowercase();
        match &self.repos {
            RepoState::Loaded(v) => {
                github::group_repos(v.iter().filter(|r| r.name_with_owner.to_ascii_lowercase().contains(&f)))
            }
            _ => Vec::new(),
        }
    }

    /// Selectable repos in display order (grouped); picker row i (1-based) is element i-1.
    pub fn picker_rows(&self, filter: &str) -> Vec<&github::RepoEntry> {
        self.picker_groups(filter).into_iter().flat_map(|(_, v)| v).collect()
    }

    /// Enter in the picker: row 0 turns GitHub off; a row saves that repo; a typed owner/repo
    /// with no matching row is checked with `gh repo view` first.
    fn pick_repo(&mut self, filter: &str, sel: usize, store: &mut Store) {
        let rows = self.picker_rows(filter);
        let typed = filter.trim();
        let choice: std::result::Result<Option<String>, String> = if sel == 0 && !(rows.is_empty() && github::valid_repo(typed)) {
            Ok(None)
        } else if let Some(r) = rows.get(sel.wrapping_sub(1)) {
            Ok(Some(r.name_with_owner.clone()))
        } else if github::valid_repo(typed) {
            github::check_repo(typed).map(Some)
        } else {
            Err(format!("no repo matches '{typed}' — type owner/repo, or esc"))
        };
        match choice {
            Err(e) => self.picker_msg = Some(e),
            Ok(repo) => {
                let r = store.set_github(repo.as_deref());
                let msg = match &repo {
                    Some(n) => format!("github: {n}"),
                    None => "github off".to_string(),
                };
                if self.report(r, |_| msg).is_some() {
                    self.mode = Mode::Normal;
                    self.picker_msg = None;
                    self.reload(store);
                }
            }
        }
    }

    /// `y` on the delete prompt: delete the card — or archive it, on an archive board.
    fn remove_confirmed(&mut self, id: i64, force: bool, store: &mut Store) {
        let actor = self.actor.clone();
        let r = store.remove_card(id, &actor, force);
        let said = |r: &crate::store::archive::Removed| {
            let verb = if r.archived { "archived" } else { "deleted" };
            format!("{verb} #{} \"{}\"", r.card.id, r.card.title)
        };
        if self.report(r, said).is_some() {
            self.reload(store);
        }
    }

    fn open_edit(&mut self, id: i64, from_popup: bool) {
        if let Some(c) = self.snap.cards.iter().find(|c| c.id == id) {
            // the holder rule: someone else's DOING card is not rewritten from here
            if let Some(owner) = c.owner.as_deref().filter(|o| c.column == "doing" && !o.eq_ignore_ascii_case(&self.actor)) {
                self.status = Some((format!("#{id} is held by {owner} — to edit it anyway use 'tb edit {id} … --force' (logged)"), true));
                return;
            }
            let title = crate::store::raw_title(c);
            let cursor = title.chars().count();
            let due = c.due.clone().unwrap_or_default();
            self.mode = Mode::Edit(EditForm {
                id,
                open_title: title.clone(),
                open_due: due.clone(),
                open_desc: c.description.clone(),
                title,
                due,
                desc: c.description.clone(),
                field: 0,
                cursor,
                from_popup,
            });
        }
    }

    /// Would moving `card` to done skip GitHub's evidence (issue/PR still open)?
    fn open_on_github(&self, id: i64) -> Option<i64> {
        let c = self.snap.cards.iter().find(|c| c.id == id)?;
        let n = c.gh_ref?;
        let snap = self.gh.snap.as_ref()?;
        github::still_open(snap, n).then_some(n)
    }

    /// On this actor's own REVIEW card: ask before forcing a close, naming every rule the force
    /// would skip (`Store::done_would_skip`, the same list the transition applies) — or, when
    /// one of them is the verifier rule, refuse outright: no prompt hands an agent without a
    /// verifier role a way past it. False when the card is not the actor's own review work, so
    /// the ordinary move (and its ordinary refusals) applies.
    fn ask_own_close(&mut self, id: i64, store: &Store) -> bool {
        if !self.is_own_review(id, store) {
            return false;
        }
        if self.verifier_refuses(id, store) {
            return true;
        }
        match store.done_would_skip(id, &self.actor) {
            Ok(skips) => self.mode = approve_own(id, &skips),
            Err(e) => self.status = Some((e.to_string(), true)),
        }
        true
    }

    /// Would the verifier rule refuse this actor closing `id`? If so, say so on the status
    /// line (true); a board that cannot tell reports that instead.
    fn verifier_refuses(&mut self, id: i64, store: &Store) -> bool {
        let refused = |c: &crate::store::Code| {
            matches!(c, crate::store::Code::NotVerifier | crate::store::Code::UnregisteredVerifier | crate::store::Code::AgentAsPerson)
        };
        match store.done_would_skip(id, &self.actor) {
            Ok(skips) if skips.iter().any(|(_, c)| refused(c)) => {
                self.status = Some((
                    format!("#{id} is your own work, and only a verifier closes a card — leave it in review for an independent verifier"),
                    true,
                ));
                true
            }
            Ok(_) => false,
            Err(e) => {
                self.status = Some((e.to_string(), true));
                true
            }
        }
    }

    /// Is `id` a REVIEW card this actor authored (their own work)?
    fn is_own_review(&self, id: i64, store: &Store) -> bool {
        store.card(id).is_ok_and(|c| c.column == "review")
            && store.author(id).ok().flatten().is_some_and(|a| a.eq_ignore_ascii_case(&self.actor))
    }

    /// Move the selected card to `target` (None = `done` semantics), asking before a
    /// done that GitHub doesn't back, and before approving your own work.
    fn move_selected(&mut self, target: Option<&str>, store: &mut Store) {
        let Some((id, column)) = self.selected().map(|c| (c.id, c.column.clone())) else { return };
        let to = match target {
            Some(t) => t.to_string(),
            None => match column.as_str() {
                "doing" => "review".into(),
                "done" => "done".into(),
                _ => "done".into(),
            },
        };
        // someone else's DOING card: ask y/n (the store refuses without --force)
        if column == "doing" {
            if let Some(c) = self.snap.cards.iter().find(|c| c.id == id) {
                let mine = c.owner.as_deref().is_none_or(|o| o.eq_ignore_ascii_case(&self.actor));
                if !mine && self.actor != "github" {
                    self.mode = Mode::Confirm {
                        prompt: format!(
                            "#{} is held by {} — move it to {} anyway? y/n (logged)",
                            id,
                            c.owner.clone().unwrap_or_default(),
                            to
                        ),
                        action: Confirm::NotMine(id, to),
                    };
                    return;
                }
            }
        }
        if to == "done" && column != "done" {
            if let Some(n) = self.open_on_github(id) {
                self.mode = Mode::Confirm {
                    action: Confirm::ForceDone(id),
                    prompt: format!("issue gh#{n} still open on GitHub — mark done anyway? y/n"),
                };
                return;
            }
            if self.ask_own_close(id, store) {
                return;
            }
        }
        if column == "review" && to == "doing" {
            self.mode = Mode::SendBack { id, buf: String::new() };
            return;
        }
        let actor = self.actor.clone();
        let r = if target.is_none() { store.done(id, &actor) } else { store.move_to(id, &to, &actor) };
        if self.report(r, |c| format!("#{} -> {}", c.id, c.column)).is_some() {
            self.reload(store);
            self.focus_card(id);
        }
    }

    fn reorder_selected(&mut self, how: &str, store: &mut Store) {
        let Some(id) = self.selected().map(|c| c.id) else { return };
        if self.col == 3 {
            return; // DONE is ordered by time
        }
        if self.guard_write(id, HeldWrite::Reorder(how.to_string())) {
            return;
        }
        self.commit_reorder(id, how, store);
    }

    fn commit_reorder(&mut self, id: i64, how: &str, store: &mut Store) {
        let actor = self.actor.clone();
        let r = store.reorder(id, how, &actor);
        // on a due-sorted column position is only the tie-break: say so instead of seeming
        // to do nothing (a board that does not set `sort due` reports exactly as before)
        let by_date = self.snap.sort.by_date(COLUMNS[self.col.min(COLUMNS.len() - 1)]);
        let said = if by_date {
            format!("#{id} moved {how} — sorted by due date: position only orders cards with the same date (or none)")
        } else {
            format!("#{id} moved {how}")
        };
        if self.report(r, |_| said).is_some() {
            self.reload(store);
            self.focus_card(id);
        }
    }

    /// Someone else's DOING card: queue this change through a y/n Confirm (like `x`/shift-arrow
    /// already do for delete/move) instead of writing it — returns `true` if a prompt was shown
    /// (the caller stops there). `check`/`prio` have no guard of their own at the store layer
    /// (only `edit`, `block`, `rm` and a column move go through `holder_check`), so the TUI
    /// applies the same rule here that `main.rs` applies at the CLI.
    fn guard_write(&mut self, id: i64, write: HeldWrite) -> bool {
        let Some(c) = self.snap.cards.iter().find(|c| c.id == id) else { return false };
        if c.column != "doing" || self.actor == "github" {
            return false;
        }
        let Some(owner) = c.owner.as_deref().filter(|o| !o.eq_ignore_ascii_case(&self.actor)) else {
            return false;
        };
        self.mode = Mode::Confirm {
            prompt: format!("#{id} is held by {owner} — {} anyway? y/n (logged)", write.what()),
            action: Confirm::NotMineWrite(id, write),
        };
        true
    }

    /// `y` on a `guard_write` prompt: the change goes through forced, then a `force` event is
    /// logged — the same two steps `main.rs` does for `check --force` / `prio --force`.
    fn commit_forced_write(&mut self, id: i64, write: HeldWrite, store: &mut Store) {
        let actor = self.actor.clone();
        let forced = store.holder_check(id, &actor, true, write.what());
        match &write {
            HeldWrite::Reorder(how) => self.commit_reorder(id, how, store),
            HeldWrite::Check(n) => self.commit_check(id, *n, store),
            HeldWrite::AddCheck(text) => self.commit_add_check(id, text, store),
            HeldWrite::RemoveCheck(n) => self.commit_remove_check(id, *n, store),
        }
        if let Ok(Some(owner)) = forced {
            let _ = store.log_forced(id, &actor, write.did(), &owner);
        }
    }

    fn commit_check(&mut self, id: i64, n: i64, store: &mut Store) {
        let actor = self.actor.clone();
        let r = store.check(id, n, &actor);
        self.report(r, |d| format!("#{id} item {n} {}", if *d { "checked" } else { "unchecked" }));
        self.reload(store);
    }

    fn commit_add_check(&mut self, id: i64, text: &str, store: &mut Store) {
        let actor = self.actor.clone();
        let r = store.add_check(id, text, &actor);
        if let Some(n) = self.report(r, |n| format!("#{id} item {n} added")) {
            self.reload(store);
            self.cursor = (n as usize).saturating_sub(1);
        }
    }

    fn commit_remove_check(&mut self, id: i64, n: i64, store: &mut Store) {
        let actor = self.actor.clone();
        let pre_len = self.popup.as_ref().map(|d| d.checklist.len()).unwrap_or(0);
        let r = store.remove_check(id, n, &actor);
        if self.report(r, |_| format!("#{id} item {n} deleted")).is_some() {
            self.reload(store);
            self.cursor = self.cursor.min(pre_len.saturating_sub(2));
        }
    }

    /// The panel area (GITHUB / AGENTS) drawn to the right of `from`, overlapping it
    /// vertically — the nearest one, top first.
    fn area_right_of(&self, from: Rect) -> Option<Focus> {
        let [g, a] = self.area_rects.get();
        let right = from.x + from.width;
        let overlaps = |r: &Rect| r.y < from.y + from.height && from.y < r.y + r.height;
        [(Focus::Github, g), (Focus::Agents, a)]
            .into_iter()
            .filter_map(|(f, r)| r.map(|r| (f, r)))
            .filter(|(_, r)| r.x >= right && overlaps(r))
            .min_by_key(|(_, r)| (r.x, r.y))
            .map(|(f, _)| f)
    }

    /// Is a column drawn to the left of `from` (overlapping it vertically)?
    fn columns_left_of(&self, from: Rect) -> bool {
        let overlaps = |r: &Rect| r.y < from.y + from.height && from.y < r.y + r.height;
        self.col_rects.get().iter().any(|r| r.width > 0 && r.x + r.width <= from.x && overlaps(r))
    }

    /// The column drawn next to column `ci` in direction (dx, dy), by last frame's geometry.
    fn col_neighbour(&self, ci: usize, dx: i32, dy: i32) -> Option<usize> {
        let rects = self.col_rects.get();
        let from = rects[ci];
        if from.width == 0 {
            return None;
        }
        let overlap_y = |r: &Rect| r.y < from.y + from.height && from.y < r.y + r.height;
        let overlap_x = |r: &Rect| r.x < from.x + from.width && from.x < r.x + r.width;
        (0..4)
            .filter(|&c| c != ci && rects[c].width > 0)
            .filter(|&c| {
                let r = rects[c];
                match (dx, dy) {
                    (1, _) => r.x >= from.x + from.width && overlap_y(&r),
                    (-1, _) => r.x + r.width <= from.x && overlap_y(&r),
                    (_, 1) => r.y >= from.y + from.height && overlap_x(&r),
                    _ => r.y + r.height <= from.y && overlap_x(&r),
                }
            })
            .min_by_key(|&c| {
                let r = rects[c];
                (r.x.abs_diff(from.x) + r.y.abs_diff(from.y), c)
            })
    }

    /// Columns stacked (or not drawn): left/right step through them by index instead.
    fn columns_stacked(&self) -> bool {
        let rects = self.col_rects.get();
        rects.iter().all(|r| r.width == 0 || r.x == rects[0].x)
    }

    fn area_rect(&self, a: Focus) -> Option<Rect> {
        let [g, ag] = self.area_rects.get();
        match a {
            Focus::Github => g,
            Focus::Agents => ag,
            Focus::Columns => None,
        }
    }

    /// Areas in Tab order: the columns, then each enabled panel.
    fn areas(&self) -> Vec<Focus> {
        let mut v = vec![Focus::Columns];
        if self.show_github {
            v.push(Focus::Github);
        }
        if self.show_agents {
            v.push(Focus::Agents);
        }
        v
    }

    /// Is `a` drawn inline (as a panel) on the board?
    fn inline(&self, a: Focus) -> bool {
        let (g, ag) = self.shown.get();
        match a {
            Focus::Columns => true,
            Focus::Github => g,
            Focus::Agents => ag,
        }
    }

    /// Go to an area: inline panels get focus on the board; the rest open full screen.
    pub fn go_area(&mut self, a: Focus) {
        self.focus = a;
        self.view = match a {
            Focus::Columns => View::Board,
            _ if self.inline(a) && self.view == View::Board => View::Board,
            _ if self.inline(a) && self.last_shape.get() != Shape::Focus => View::Board,
            Focus::Github => View::Github,
            Focus::Agents => View::Agents,
        };
    }

    /// Tab / Shift-Tab: the next / previous area.
    fn cycle_focus(&mut self, back: bool) {
        let order = self.areas();
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len();
        self.go_area(order[if back { (i + n - 1) % n } else { (i + 1) % n }]);
    }

    /// Up/down between areas on the board (panels or their bars), without paging.
    fn step_area(&mut self, down: bool) {
        let order = self.areas();
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let j = if down { (i + 1).min(order.len() - 1) } else { i.saturating_sub(1) };
        if j != i {
            self.focus = order[j];
            if down {
                self.gh_sel = 0;
                self.ag_sel = 0;
            }
        }
    }

    /// Selectable GitHub rows: 1 (repo / "no repo") + open PRs + issues.
    pub fn gh_rows(&self) -> usize {
        match &self.gh.snap {
            Some(s) if self.gh.repo.is_some() => 1 + s.prs.len() + s.issues.len(),
            _ => 1,
        }
    }

    /// What GitHub row `i` is: None = the repo row, Some((pr?, number)).
    pub fn gh_row(&self, i: usize) -> Option<(bool, i64)> {
        let s = self.gh.snap.as_ref().filter(|_| self.gh.repo.is_some())?;
        if i == 0 {
            return None;
        }
        if i <= s.prs.len() {
            return Some((true, s.prs[i - 1].number));
        }
        let f = github::factory(s, &self.snap.cards, self.snap.now);
        f.issues.get(i - 1 - s.prs.len()).map(|r| (false, r.number))
    }

    /// Who is on this board, then the herdr agents that are not (`roster`). The AGENTS panel,
    /// its bar, the header count and the row keys all read this one list.
    pub fn roster(&self) -> crate::roster::Roster<'_> {
        crate::roster::roster(&self.snap, agent_list(self))
    }

    /// The card of AGENTS row `i` (the one it holds or reviews).
    fn agent_card(&self, i: usize) -> Option<&Card> {
        self.roster().here.get(i).and_then(|r| r.card)
    }

    fn open_picker(&mut self, preselect: bool) {
        self.picker_msg = None;
        self.start_repo_load();
        self.picker_preselect = preselect;
        self.mode = Mode::Picker { filter: String::new(), sel: 0 };
    }

    /// `B`: read the boards (and their counts) fresh, then open the overlay on the current
    /// board. With `TB_DB` there is nothing to choose between, so it refuses on the footer
    /// instead of opening an overlay that cannot do anything.
    pub fn open_boards(&mut self) {
        match crate::boards::picker_rows(&self.actor) {
            Err(msg) => self.status = Some((msg, true)),
            Ok(rows) => {
                let sel = rows.iter().position(|b| b.name == self.snap.board).unwrap_or(0);
                self.boards = rows;
                // one row per archived name: its newest archive (the one `restore` takes)
                let mut archived = crate::boards::archived();
                archived.reverse();
                archived.dedup_by(|a, b| a.name == b.name);
                archived.reverse();
                self.archived_boards = archived;
                self.mode = Mode::Boards { sel };
            }
        }
    }

    /// `*` in the board picker: the selected board becomes the default — the board a plain
    /// `tb` opens — at once, no question, saved exactly as `tb boards --default NAME` saves it.
    /// `*` on the built-in `default` board goes back to it (clears the saved choice, like
    /// `--default --clear`). An archived board cannot be the default. The list is re-read, so
    /// its `*` mark moves, and every rule that reads the default (the picker's own refusals
    /// among them) follows the new one.
    fn make_default(&mut self, sel: usize) {
        let name = match self.picked(sel) {
            Picked::Live(row) => row.name,
            Picked::Archived(name) => {
                self.status = Some((format!("'{name}' is archived — restore it (r) before making it the default"), true));
                return;
            }
            Picked::None => return,
        };
        // what is saved (TB_BOARD in this shell does not count: it is not what `*` changes)
        let saved = crate::boards::saved_default().ok().flatten();
        if saved.as_deref().unwrap_or(crate::boards::DEFAULT_BOARD) == name {
            self.status = Some((format!("'{name}' is already the default"), false));
            return;
        }
        let status = match crate::boards::set_default(Some(&name)) {
            Err(e) => (e.to_string(), true),
            // what a plain `tb` opens now: TB_BOARD in this environment still beats the setting
            Ok(()) => match crate::boards::plain_board() {
                Ok((opens, crate::boards::DefaultSource::Env)) if opens != name => {
                    (format!("'{name}' saved as the default — TB_BOARD={opens} still wins in this shell"), false)
                }
                _ => (format!("'{name}' is now the default — a plain 'tb' opens it"), false),
            },
        };
        self.open_boards();
        if let Mode::Boards { .. } = self.mode {
            if let Some(sel) = self.boards.iter().position(|b| b.name == name) {
                self.mode = Mode::Boards { sel };
            }
        }
        self.status = Some(status);
    }

    /// The board picker's row `i`: a live board, then the archived ones.
    fn picked(&self, i: usize) -> Picked {
        if let Some(row) = self.boards.get(i) {
            return Picked::Live(row.clone());
        }
        match self.archived_boards.get(i - self.boards.len()) {
            Some(a) => Picked::Archived(a.name.clone()),
            None => Picked::None,
        }
    }

    /// `a` / `r` / `d` in the board picker: done at once, no question — `a` archives, `d`
    /// deletes (a live board is archived and deleted in one go), `r` restores — and a status
    /// line says what happened. The rules are the CLI's, through the same `boards` functions:
    /// the board a bare `tb` opens and a board another `tb` has open are refused, on the
    /// status line. The board this picker is on is refused too (switch to another first): it
    /// is the one board archiving would pull out from under the running `tb`.
    fn board_key(&mut self, key: char, sel: usize) {
        let (act, name) = match (key, self.picked(sel)) {
            ('a' | 'd', Picked::Live(row)) if row.name == self.snap.board => {
                let what = if key == 'a' { "archive" } else { "delete" };
                self.status = Some((format!("you are on '{}' — switch to another board first, then {what} it", row.name), true));
                return;
            }
            ('a', Picked::Live(row)) => (BoardAct::Archive, row.name),
            ('d', Picked::Live(row)) => (BoardAct::DeleteLive, row.name),
            ('d', Picked::Archived(name)) => (BoardAct::Delete, name),
            ('r', Picked::Archived(name)) => (BoardAct::Restore, name),
            ('r', Picked::Live(row)) => {
                self.status = Some((format!("'{}' is not archived — nothing to restore", row.name), true));
                return;
            }
            ('a', Picked::Archived(name)) => {
                self.status = Some((format!("'{name}' is already archived — r restores it, d deletes it"), true));
                return;
            }
            _ => return,
        };
        // The board a bare `tb` opens: refused by `boards::archive` in its own words, which
        // say "archive" — `d` says "delete".
        if matches!(act, BoardAct::DeleteLive) && name == crate::boards::default_name() {
            self.status = Some((format!("'{name}' is the board a bare 'tb' opens — delete another board, or point TB_BOARD elsewhere first"), true));
            return;
        }
        // A board another `tb` has open: archive/restore would wait for it to close (up to
        // 10 s) before refusing, the picker frozen and every key typed meanwhile landing on
        // the re-read list. Look first, without waiting, and refuse at once by name.
        // (Not for an archived board being deleted: nothing opens an archived file.)
        let live = crate::lock::sibling(&crate::boards::path_for(&name));
        if !matches!(act, BoardAct::Delete)
            && matches!(crate::lock::take(&live, crate::lock::Mode::Exclusive, Duration::ZERO), Err(crate::lock::Error::Busy(_)))
        {
            self.status = Some((format!("'{name}' is open in another tb — close it there, then try again"), true));
            return;
        }
        let deleted = |d: crate::boards::Deleted| format!("deleted '{name}' ({} file(s))", d.removed.len());
        let r = match act {
            BoardAct::Archive => crate::boards::archive(&name).map(|_| format!("archived '{name}' — r restores it")),
            BoardAct::Restore => crate::boards::restore(&name).map(|_| format!("restored '{name}'")),
            BoardAct::Delete => crate::boards::delete(&name, false).map(deleted),
            // a live board: archived first (the same checks as `a`), then deleted for good
            BoardAct::DeleteLive => crate::boards::archive(&name).and_then(|_| crate::boards::delete(&name, false)).map(deleted),
        };
        let status = match r {
            Ok(msg) => (msg, false),
            Err(e) => (e.to_string(), true),
        };
        // back to the picker, re-read from disk, on the same board if it is still listed
        self.open_boards();
        if let Mode::Boards { .. } = self.mode {
            let at = self
                .boards
                .iter()
                .position(|b| b.name == name)
                .or_else(|| self.archived_boards.iter().position(|a| a.name == name).map(|i| i + self.boards.len()));
            if let Some(sel) = at {
                self.mode = Mode::Boards { sel };
            }
        }
        self.status = Some(status);
    }

    /// Enter in the board picker: open that board's file in place of the running one — no
    /// restart. Everything the board owns (its cards, WIP limit, theme, layout, GitHub repo
    /// and panel settings) comes back through `reload`, so the header and both panels follow.
    pub fn switch_board(&mut self, row: &crate::boards::BoardRow, store: &mut Store) {
        self.mode = Mode::Normal;
        if row.name == self.snap.board {
            return;
        }
        match Store::open(&row.path) {
            Err(e) => self.status = Some((e.to_string(), true)),
            Ok(s) => {
                *store = s.named(&row.name);
                self.popup = None;
                self.col = 0;
                self.row = [0; 4];
                self.cursor = 0;
                self.focus = Focus::Columns;
                self.view = View::Board;
                self.focus_nav = false;
                self.gh_sel = 0;
                self.ag_sel = 0;
                // the old board's GitHub cache and repo list belong to the old board
                self.gh = GhView::default();
                self.repos = RepoState::Idle;
                self.repos_rx = None;
                self.picker_msg = None;
                // cleared first, so `reload` can surface a warning this open just raised (a
                // wide file, a backup made upgrading an older board) instead of it sitting
                // queued behind whatever the old board was last showing (card #83)
                self.status = None;
                self.status_until = None;
                self.reload(store);
                if self.status.is_none() {
                    self.status = Some((format!("board: {}", row.name), false));
                    self.status_until = Some(Instant::now() + Duration::from_secs(3));
                }
            }
        }
    }

    /// Keys while a panel (GITHUB / AGENTS) has focus. Card keys do nothing here.
    fn panel_key(&mut self, key: KeyEvent, store: &mut Store) -> bool {
        let (g_shown, a_shown) = self.shown.get();
        let in_view = self.view != View::Board;
        let enabled = self.areas().contains(&self.focus);
        if !enabled {
            self.focus = Focus::Columns;
            self.view = View::Board;
            return self.normal_key(key, store);
        }
        // on the board, a panel shown only as a bar — enter opens it full screen
        if !in_view && !self.inline(self.focus) {
            match key.code {
                KeyCode::Enter => {
                    self.view = if self.focus == Focus::Github { View::Github } else { View::Agents };
                    return false;
                }
                KeyCode::Up => {
                    self.step_area(false);
                    return false;
                }
                KeyCode::Down => {
                    self.step_area(true);
                    return false;
                }
                _ => {}
            }
        }
        let a_shown = a_shown || self.bars.get().1;
        let _ = g_shown;
        let n_agents = self.roster().panel_rows();
        match (self.focus, key.code) {
            (_, KeyCode::Char('q')) => return true,
            (_, KeyCode::Esc) => {
                self.status = None;
                self.focus = Focus::Columns;
                self.view = View::Board;
            }
            (_, KeyCode::Tab) => self.cycle_focus(false),
            (_, KeyCode::BackTab) => self.cycle_focus(true),
            (_, KeyCode::Char('?')) => self.mode = Mode::Help,
            (Focus::Github, KeyCode::Up) => {
                if self.gh_sel > 0 {
                    self.gh_sel -= 1;
                } else if !in_view {
                    self.focus = Focus::Columns;
                }
            }
            (Focus::Github, KeyCode::Down) => {
                if self.gh_sel + 1 < self.gh_rows() {
                    self.gh_sel += 1;
                } else if a_shown && !in_view {
                    self.focus = Focus::Agents;
                    self.ag_sel = 0;
                }
            }
            (Focus::Github, KeyCode::Enter) => match self.gh_row(self.gh_sel) {
                None => self.open_picker(true),
                Some((pr, number)) => self.mode = Mode::GhItem { pr, number },
            },
            (Focus::Agents, KeyCode::Up) => {
                if self.ag_sel > 0 {
                    self.ag_sel -= 1;
                } else if !in_view {
                    self.step_area(false);
                    if self.focus == Focus::Github {
                        self.gh_sel = self.gh_rows().saturating_sub(1);
                    }
                }
            }
            (Focus::Github | Focus::Agents, KeyCode::Left)
                if !in_view && self.area_rect(self.focus).is_some_and(|r| self.columns_left_of(r)) =>
            {
                // back to the column (and card) you came from
                self.focus = Focus::Columns;
            }
            (Focus::Agents, KeyCode::Down) => self.ag_sel = (self.ag_sel + 1).min(n_agents.saturating_sub(1)),
            (Focus::Agents, KeyCode::Enter) => {
                if self.ag_sel < n_agents {
                    self.mode = Mode::AgentInfo(self.ag_sel);
                }
            }
            // board-wide keys still work; card keys (a d > < n) do nothing in a panel
            (_, KeyCode::Char(c @ ('T' | 'L' | 'A' | 'G' | 'R' | 'B'))) => {
                let prev = self.focus;
                self.focus = Focus::Columns;
                let q = self.normal_key(KeyEvent::new(KeyCode::Char(c), key.modifiers), store);
                if !matches!(self.mode, Mode::Picker { .. } | Mode::Boards { .. }) {
                    self.focus = prev;
                }
                return q;
            }
            _ => {}
        }
        false
    }

    /// Issue/PR popup `a`: add a card `repo: gh#N short title` in TODO, once.
    fn add_gh_card(&mut self, pr: bool, number: i64, store: &mut Store) {
        if let Some(c) = self.snap.cards.iter().find(|c| c.gh_ref == Some(number)) {
            self.status = Some((format!("already on board as #{}", c.id), false));
            return;
        }
        let Some(s) = &self.gh.snap else { return };
        let title = if pr {
            s.prs.iter().find(|p| p.number == number).map(|p| p.title.clone())
        } else {
            s.issues.iter().find(|i| i.number == number).map(|i| i.title.clone())
        }
        .unwrap_or_default();
        let tag: String = s
            .repo
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(20)
            .collect();
        let short = github::short_title(&title);
        let raw = if tag.is_empty() { format!("gh#{number} {short}") } else { format!("{tag}: gh#{number} {short}") };
        let actor = self.actor.clone();
        let r = store.add(&raw, "", &[], &actor);
        if let Some(id) = self.report(r, |id| format!("added #{id} (gh#{number})")) {
            self.reload(store);
            let _ = id;
        }
    }

    /// Issue/PR popup `o`: `gh browse N -R owner/repo`, detached.
    fn open_in_browser(&mut self, _pr: bool, number: i64) {
        let Some(repo) = self.gh.repo.clone() else { return };
        let bin = crate::env("GH").unwrap_or_else(|| "gh".into());
        let spawned = std::process::Command::new(bin)
            .args(["browse", &number.to_string(), "-R", &repo])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        self.status = Some(match spawned {
            Ok(_) => (format!("opening #{number} in the browser"), false),
            Err(e) => (format!("cannot run gh browse: {e}"), true),
        });
    }

    /// Sidebar: up/down walk every card across the stacked sections.
    fn sidebar_step(&mut self, down: bool) {
        let n = self.col_cards(self.col).len();
        let r = self.row[self.col].min(n.saturating_sub(1));
        if down && r + 1 < n {
            self.row[self.col] = r + 1;
            return;
        }
        if !down && r > 0 && n > 0 {
            self.row[self.col] = r - 1;
            return;
        }
        let order: Vec<usize> = if down { (self.col + 1..4).collect() } else { (0..self.col).rev().collect() };
        if let Some(c) = order.into_iter().find(|c| !self.col_cards(*c).is_empty()) {
            self.col = c;
            self.row[c] = if down { 0 } else { self.col_cards(c).len() - 1 };
        } else if down {
            // past the last card: into GITHUB / AGENTS below
            if let Some(a) = self.areas().get(1).copied() {
                self.focus = a;
                self.gh_sel = 0;
                self.ag_sel = 0;
            }
        }
    }

    /// The FOCUS view's card: once the user has moved, the selection; before that the card the
    /// current user holds in DOING, else the top TODO card, else the selection.
    pub fn focus_target(&self) -> Option<&Card> {
        if self.focus_nav {
            return self.selected();
        }
        let me = self.actor.to_ascii_lowercase();
        let mine = self.col_cards(1).into_iter().find(|c| c.owner.as_deref().is_some_and(|o| o.to_ascii_lowercase() == me));
        mine.or_else(|| self.col_cards(0).first().copied()).or_else(|| self.selected())
    }

    /// The FOCUS card's checklist (from the board snapshot's cache of card details).
    pub fn focus_checklist(&self, id: i64) -> Vec<(i64, String, bool)> {
        self.focus_items.borrow().get(&id).cloned().unwrap_or_default()
    }

    /// Checklist cursor for the FOCUS card: the first unticked item.
    fn focus_first_open(&self, store: &Store, id: i64) -> usize {
        store.show(id).map(|d| d.checklist.iter().position(|i| !i.done).unwrap_or(0)).unwrap_or(0)
    }

    /// FOCUS view keys: <- -> previous/next card in the column, up/down switch column,
    /// enter ticks the checklist item under the cursor and moves to the next open one.
    fn focus_key(&mut self, key: KeyEvent, store: &mut Store) -> Option<bool> {
        if !self.focus_nav {
            if let Some(id) = self.focus_target().map(|c| c.id) {
                self.focus_card(id);
                self.cursor = self.focus_first_open(store, id);
            }
            self.focus_nav = true;
        }
        match key.code {
            // shift+left/right moves the card — the same keys mean the same thing in every
            // view (the focus view previously swallowed them as navigation)
            KeyCode::Left | KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let (_, column) = self.selected().map(|c| (c.id, c.column.clone()))?;
                let ci = COLUMNS.iter().position(|k| *k == column).unwrap_or(0);
                let to = if key.code == KeyCode::Left { ci.checked_sub(1) } else { (ci < 3).then_some(ci + 1) };
                if let Some(t) = to {
                    self.move_selected(Some(COLUMNS[t]), store);
                }
                return Some(false);
            }
            KeyCode::Up | KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.reorder_selected(if key.code == KeyCode::Up { "up" } else { "down" }, store);
                return Some(false);
            }
            KeyCode::Left | KeyCode::Right => {
                let n = self.col_cards(self.col).len();
                if n > 0 {
                    let r = self.row[self.col].min(n - 1);
                    self.row[self.col] = if key.code == KeyCode::Right { (r + 1) % n } else { (r + n - 1) % n };
                }
            }
            KeyCode::Up | KeyCode::Down => {
                // the next non-empty column (wrapping)
                for step in 1..=4 {
                    let c = if key.code == KeyCode::Down { (self.col + step) % 4 } else { (self.col + 4 - step) % 4 };
                    if !self.col_cards(c).is_empty() {
                        self.col = c;
                        break;
                    }
                }
            }
            KeyCode::Enter => {
                let Some(id) = self.selected().map(|c| c.id) else { return Some(false) };
                let items = store.show(id).map(|d| d.checklist).unwrap_or_default();
                if let Some(item) = items.get(self.cursor.min(items.len().saturating_sub(1))) {
                    let n = item.idx;
                    if self.guard_write(id, HeldWrite::Check(n)) {
                        return Some(false);
                    }
                    self.commit_check(id, n, store);
                    self.cursor = self.focus_first_open(store, id);
                }
            }
            _ => return None,
        }
        if let Some(id) = self.selected().map(|c| c.id) {
            if matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down) {
                self.cursor = self.focus_first_open(store, id);
            }
        }
        Some(false)
    }

    fn normal_key(&mut self, key: KeyEvent, store: &mut Store) -> bool {
        if self.last_shape.get() == Shape::Focus && self.view == View::Board {
            if let Some(q) = self.focus_key(key, store) {
                return q;
            }
        }
        let actor = self.actor.clone();
        let sel = self.selected().map(|c| (c.id, c.column.clone()));
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => self.status = None,
            KeyCode::Left | KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let Some((_, column)) = sel else { return false };
                let ci = COLUMNS.iter().position(|k| *k == column).unwrap_or(0);
                let to = if key.code == KeyCode::Left { ci.checked_sub(1) } else { (ci < 3).then_some(ci + 1) };
                if let Some(t) = to {
                    self.move_selected(Some(COLUMNS[t]), store);
                }
            }
            KeyCode::Up | KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.reorder_selected(if key.code == KeyCode::Up { "up" } else { "down" }, store)
            }
            KeyCode::Char('K') => self.reorder_selected("up", store),
            KeyCode::Char('J') => self.reorder_selected("down", store),
            KeyCode::Char('e') => {
                if let Some((id, _)) = sel {
                    self.open_edit(id, false);
                }
            }
            KeyCode::Char('x') => {
                if let Some(c) = self.selected() {
                    // an archive board archives; someone else's DOING card is named as held
                    let verb = if store.rm_mode().is_ok_and(|m| m == crate::store::archive::RM_ARCHIVE) { "archive" } else { "delete" };
                    let holder = c.owner.as_deref().filter(|o| c.column == "doing" && !o.eq_ignore_ascii_case(&self.actor));
                    self.mode = match holder {
                        Some(owner) => Mode::Confirm {
                            action: Confirm::DeleteHeld(c.id),
                            prompt: format!("#{} is held by {owner} — {verb} it anyway? y/n (logged)", c.id),
                        },
                        None => Mode::Confirm {
                            action: Confirm::Delete(c.id),
                            prompt: format!("{verb} #{} \"{}\"? y/n", c.id, c.title),
                        },
                    };
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Left | KeyCode::Right => {
                let dx = if key.code == KeyCode::Right { 1 } else { -1 };
                if let Some(c) = self.col_neighbour(self.col, dx, 0) {
                    self.col = c;
                } else if dx == 1 && self.area_right_of(self.col_rects.get()[self.col]).is_some() {
                    // e.g. third-h: GITHUB / AGENTS sit to the right of DONE
                    self.focus = self.area_right_of(self.col_rects.get()[self.col]).unwrap_or(Focus::Columns);
                    self.gh_sel = 0;
                    self.ag_sel = 0;
                } else if self.columns_stacked() {
                    // stacked sections: step through the columns
                    let wrap = self.last_shape.get() == Shape::Focus;
                    self.col = match (dx, self.col) {
                        (1, 3) if wrap => 0,
                        (-1, 0) if wrap => 3,
                        (1, c) => (c + 1).min(3),
                        (_, c) => c.saturating_sub(1),
                    };
                }
            }
            KeyCode::Up | KeyCode::Down if self.last_shape.get() == Shape::ThirdV => {
                self.sidebar_step(key.code == KeyCode::Down)
            }
            KeyCode::Up if self.row[self.col] == 0 && self.col_neighbour(self.col, 0, -1).is_some() => {
                // half-v grid: up from the top card goes to the column above (its last card)
                let c = self.col_neighbour(self.col, 0, -1).unwrap_or(self.col);
                self.col = c;
                self.row[c] = self.col_cards(c).len().saturating_sub(1);
            }
            KeyCode::Up => self.row[self.col] = self.row[self.col].saturating_sub(1),
            KeyCode::Down if {
                let n = self.col_cards(self.col).len();
                (n == 0 || self.row[self.col] + 1 >= n) && self.col_neighbour(self.col, 0, 1).is_some()
            } =>
            {
                // half-v grid: down from the last card goes to the column below
                let c = self.col_neighbour(self.col, 0, 1).unwrap_or(self.col);
                self.col = c;
                self.row[c] = 0;
            }
            KeyCode::Down => {
                let n = self.col_cards(self.col).len();
                let at_end = n == 0 || self.row[self.col] + 1 >= n;
                let next = self.areas().get(1).copied();
                if let (true, Some(a)) = (at_end, next) {
                    self.focus = a;
                    self.gh_sel = 0;
                    self.ag_sel = 0;
                } else {
                    self.row[self.col] = (self.row[self.col] + 1).min(n.saturating_sub(1));
                }
            }
            KeyCode::Tab => self.cycle_focus(false),
            KeyCode::BackTab => self.cycle_focus(true),
            KeyCode::Char('L') => {
                let cur = crate::store::LAYOUTS.iter().position(|l| *l == self.snap.layout).unwrap_or(0);
                let next = crate::store::LAYOUTS[(cur + 1) % crate::store::LAYOUTS.len()];
                let r = store.set_layout(next);
                if self.report(r, |_| format!("view: {next}")).is_some() {
                    self.status_until = Some(Instant::now() + Duration::from_secs(3));
                    self.reload(store);
                }
            }
            KeyCode::Char('a') => self.mode = Mode::Add(String::new()),
            // A / G toggle and remember the panels (same as `tb config agents-panel|github-panel`)
            KeyCode::Char(c @ ('A' | 'G')) => {
                let (key, now_shown) = if c == 'A' {
                    ("agents-panel", !self.show_agents)
                } else {
                    ("github-panel", !self.show_github)
                };
                if c == 'A' {
                    self.show_agents = now_shown;
                } else {
                    self.show_github = now_shown;
                }
                let _ = store.set_panel(key, if now_shown { "shown" } else { "hidden" });
            }
            KeyCode::Char('R') => self.open_picker(true),
            KeyCode::Char('B') => self.open_boards(),
            KeyCode::Char(c @ ('+' | '=' | '-')) if self.col == 1 => {
                let wip = self.snap.wip;
                let next = if c == '-' { wip - 1 } else { wip + 1 }.clamp(1, crate::store::MAX_WIP);
                if next != wip {
                    let r = store.change_wip(next, &actor);
                    if self.report(r, |_| format!("wip {wip} -> {next}")).is_some() {
                        self.reload(store);
                    }
                }
            }
            KeyCode::Char('T') => {
                let next = if self.snap.theme == "light" { "dark" } else { "light" };
                let r = store.set_theme(next);
                if self.report(r, |_| format!("theme {next}")).is_some() {
                    self.reload(store);
                }
            }
            KeyCode::Enter => {
                if let Some((id, _)) = sel {
                    self.popup = store.show(id).ok();
                    self.cursor = 0;
                    self.mode = Mode::Popup(id);
                }
            }
            KeyCode::Char('n') => {
                if let Some((id, _)) = sel {
                    self.mode = Mode::Note { id, buf: String::new(), from_popup: false };
                }
            }
            KeyCode::Char(c @ ('>' | '<' | 'd')) => {
                let Some((_, column)) = sel else { return false };
                let ci = COLUMNS.iter().position(|k| *k == column).unwrap_or(0);
                match c {
                    '>' if ci < 3 => self.move_selected(Some(COLUMNS[ci + 1]), store),
                    '<' if ci > 0 => self.move_selected(Some(COLUMNS[ci - 1]), store),
                    'd' => self.move_selected(None, store),
                    _ => {}
                }
            }
            _ => {}
        }
        false
    }
}

// ---------- drawing ----------

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn agent_list(app: &App) -> &[Agent] {
    match &app.agents {
        AgentsState::Agents(a) => a,
        _ => &[],
    }
}

fn header(app: &App, width: u16) -> Line<'static> {
    let r = app.roster();
    let base = format!(
        " TERMINAL BOARD · {} · {} cards · {} agents",
        if app.snap.board.is_empty() { "default" } else { &app.snap.board },
        app.snap.cards.len(),
        r.total()
    );
    // nobody on this board and no agent pane anywhere: the line reads as it always has
    let count = if r.total() == 0 {
        " (0 working, 0 idle)".to_string()
    } else {
        format!(" ({} here, {} elsewhere)", r.here.len(), r.elsewhere.len())
    };
    let mut left = format!("{base}{count}");
    let mut right = format!("refreshed {} ", clock_secs(app.snap.now));
    if left.chars().count() + right.chars().count() > width as usize {
        right.clear(); // no room: drop the clock rather than cut it
    }
    if r.total() > 0 && left.chars().count() > width as usize {
        left = base; // still no room: the count goes whole, never `(3 here, 2 els…`
    }
    let left = fit(&left, width as usize);
    let pad = (width as usize).saturating_sub(left.chars().count() + right.chars().count());
    Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, dim()),
    ])
}

fn clock_secs(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Lines of one card. `boxed`: inside its own box (no indent; selection = bold, the thick box
/// carries the highlight). Compact: indented, selection = reversed + bold.
fn card_lines(app: &App, card: &Card, selected: bool, width: usize, boxed: bool) -> Vec<Line<'static>> {
    let hl = match (selected, boxed) {
        (true, true) => bold(),
        (true, false) => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        _ => Style::default(),
    };
    let indent = if boxed { "" } else { "    " };
    let id = format!("#{} ", card.id);
    // narrow boxes (< 30 cols): `gh#N` moves to the meta line so the title gets the width
    let narrow = boxed && width + 4 < NARROW_CARD;
    let shown = crate::store::shown_ref(card);
    let gh = if narrow { String::new() } else { shown.map(|n| format!("gh#{n} ")).unwrap_or_default() };
    let meta_gh = if narrow { shown.map(|n| format!("gh#{n} · ")).unwrap_or_default() } else { String::new() };
    let room = width.saturating_sub(id.chars().count() + gh.chars().count());
    let mut first = vec![Span::styled(id, hl)];
    if !gh.is_empty() {
        first.push(Span::styled(gh, hl));
    }
    first.push(Span::styled(fit(&card.title, room), hl));
    let mut lines = vec![Line::from(first)];

    let parts = meta_parts(card, &app.snap, width.saturating_sub(indent.len() + meta_gh.chars().count()));
    let dropped = parts.base.trim().is_empty() && parts.mark.is_empty() && parts.warn.is_empty() && parts.quiet.is_empty();
    let (base, warn, q) = (parts.base, parts.warn, parts.quiet);
    let owner_style = match owner_agent(app, card) {
        Some(a) if a.status == "working" => Style::default(),
        Some(a) if a.status == "blocked" => bold(),
        _ => dim(),
    };
    let sep = if base.is_empty() || (parts.mark.is_empty() && warn.is_empty() && q.is_empty()) { "" } else { " " };
    let mut second: Vec<Span<'static>> = vec![Span::raw(indent)];
    if !meta_gh.is_empty() {
        second.push(Span::raw(meta_gh.clone()));
    }
    second.push(Span::styled(base, owner_style));
    second.push(Span::raw(sep));
    // the due mark: loud (bold), and red — the colour of a real problem — only once overdue
    if !parts.mark.is_empty() {
        second.push(Span::styled(parts.mark.clone(), due_mark_style(parts.overdue)));
        if !warn.is_empty() {
            second.push(Span::raw(" "));
        }
    }
    if !warn.is_empty() {
        second.push(Span::styled(warn.clone(), red()));
    }
    if !q.is_empty() {
        second.push(Span::raw(if warn.is_empty() && parts.mark.is_empty() { "" } else { " " }));
        second.push(Span::styled(q, dim()));
    }
    // A boxed card is drawn whole: when the fitting above gave up every field (none fits
    // whole in `width`), the info is cut to the width with `…` rather than left out — an empty
    // line inside a card box reads as a card with its info missing.
    if boxed && dropped {
        let all = meta_parts(card, &app.snap, 10_000);
        let text = [all.base, all.mark, all.warn, all.quiet].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            second = vec![Span::raw(meta_gh.clone()), Span::styled(fit(&text, width.saturating_sub(meta_gh.chars().count())), owner_style)];
        }
    }
    lines.push(Line::from(second));
    if card.column == "doing" {
        if let Some(n) = app.snap.last_note.get(&card.id) {
            lines.push(Line::from(Span::styled(
                format!("{indent}\"{}\"", fit(n, width.saturating_sub(indent.len() + 2))),
                Style::default().add_modifier(Modifier::ITALIC),
            )));
        }
    }
    lines
}

/// The due mark's look: bold, and red only when the card is overdue.
pub(crate) fn due_mark_style(overdue: bool) -> Style {
    if overdue {
        red().add_modifier(Modifier::BOLD)
    } else {
        bold()
    }
}

/// Display width of `s` in terminal cells (a CJK character or an emoji takes two).
pub(crate) fn cells(s: &str) -> usize {
    Span::raw(s).width()
}

/// The words of `label` that fit in `room` cells, whole: a label is never cut inside a word.
/// None when not even its first word fits.
pub(crate) fn label_words(label: &str, room: usize) -> Option<String> {
    let mut out = String::new();
    for w in label.split_whitespace() {
        let next = if out.is_empty() { w.to_string() } else { format!("{out} {w}") };
        if cells(&next) > room {
            break;
        }
        out = next;
    }
    (!out.is_empty()).then_some(out)
}

/// A column's name for a header with `room` cells for it. Without a label this is the name
/// tb always drew, whatever the room. A label is shown in whole words, as many as fit (then
/// ` today` on DONE if that fits too); when not even its first word fits, the plain name is.
pub(crate) fn column_name(snap: &Snapshot, col: &str, room: usize) -> String {
    let plain = if col == "done" { "DONE today".to_string() } else { col.to_ascii_uppercase() };
    let Some(label) = snap.display.label(col) else { return plain };
    match label_words(&label, room) {
        Some(l) if col == "done" && cells(&l) + 6 <= room => format!("{l} today"),
        Some(l) => l,
        None => plain,
    }
}

/// ` by due` for the header of a column the board orders by due date, when `room` cells are
/// left for it; nothing otherwise (it is the first thing a narrow header gives up).
pub(crate) fn date_order_note(snap: &Snapshot, col: &str, room: usize) -> &'static str {
    const NOTE: &str = "by due ";
    if snap.display.date_ordered(col) && room >= NOTE.len() {
        NOTE
    } else {
        ""
    }
}

/// The live agent behind a card's owner TEXT — same exact-name rule the AGENTS panel roster
/// uses (`herdr::exact_owner`), so a card is never coloured as if a near-miss pane held it
/// (card #93).
fn owner_agent<'a>(app: &'a App, card: &Card) -> Option<&'a Agent> {
    herdr::exact_owner(agent_list(app), card)
}

/// Does column `ci` in `area` need the dense (title-in-border) card style to fit?
fn column_needs_dense(app: &App, ci: usize, area: Rect) -> bool {
    let cards = app.col_cards(ci);
    let inner_h = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2);
    if cards.is_empty() || inner_h < 3 || inner_w < 8 {
        return false;
    }
    natural_rows(app, ci, inner_w.saturating_sub(4) as usize, false) > inner_h
}

/// The narrowest card TEXT a boxed card is drawn with (the column's inner width less the
/// card's frame and padding). Below it an info line keeps too little to read — `x blocke…`,
/// or nothing — so the frame's plan draws every card as one line instead.
pub const MIN_BOX_TEXT: u16 = 12;

/// How EVERY column box draws its cards in one frame. One plan per render, so no column is
/// ever drawn in a different card form from its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPlan {
    /// Every column shows ALL of its cards whole, each as tall as it is (`dense`: title in the box's top border, everywhere, when any column needs it).
    Natural { dense: bool },
    /// Something would be cut short: every card is a 3-row box (title in its top border, the
    /// meta line inside) and every box holds the same number of them. Rows that cannot take
    /// another 3-row box take one-line cards, so no row sits empty while cards are hidden.
    Boxed,
    /// Boxes too short for three 3-row cards: one line per card (`#32 title…  x`), so a
    /// small screen still shows several cards a box, not one.
    Line,
}

/// Rows every card of column `ci` takes as boxes `text_w` wide (dense: one row less each).
fn natural_rows(app: &App, ci: usize, text_w: usize, dense: bool) -> usize {
    app.col_cards(ci).iter().map(|c| card_lines(app, c, false, text_w, true).len() + 2 - usize::from(dense)).sum()
}

/// THE CARD RULE, decided once per frame for all four boxes `cols` (column, box rect):
///
/// 1. If every column can show ALL its cards whole in their natural boxes, it does:
///    `Natural`, dense everywhere when any column needs it — and then nothing is hidden.
/// 2. Otherwise every box uses the SAME fixed card form, so equal boxes hold an equal number
///    of cards: 3-row boxes when the shortest box fits at least two of them and three cards
///    in all (the rows under the boxes take one-line cards, the last row `+N more`), else
///    one line per card.
///
/// A card is drawn whole in its form or not at all, and a box with hidden cards ends in one
/// `+N more` line at its bottom, with no empty row above it.
pub fn card_plan(app: &App, cols: &[(usize, Rect)]) -> CardPlan {
    let inner = |r: &Rect| (r.height.saturating_sub(2) as usize, r.width.saturating_sub(2));
    // too narrow for a readable card box: one line per card, in every box
    if cols.iter().any(|(ci, r)| !app.col_cards(*ci).is_empty() && inner(r).1 < MIN_BOX_TEXT + 4) {
        return CardPlan::Line;
    }
    let natural = cols.iter().all(|(ci, r)| {
        let (h, w) = inner(r);
        app.col_cards(*ci).is_empty() || (h >= 3 && w >= 8 && natural_rows(app, *ci, w.saturating_sub(4) as usize, true) <= h)
    });
    if natural {
        let dense = cols.iter().any(|(ci, r)| column_needs_dense(app, *ci, *r));
        return CardPlan::Natural { dense };
    }
    let h = cols.iter().map(|(_, r)| inner(r).0).min().unwrap_or(0);
    let w = cols.iter().map(|(_, r)| inner(r).1).min().unwrap_or(0);
    // boxed while a box holds at least two 3-row cards and still three cards in all
    let (boxes, total) = fill_slots(h, usize::MAX, true);
    if w >= 8 && boxes >= 2 && total >= 3 {
        CardPlan::Boxed
    } else {
        CardPlan::Line
    }
}

/// Remember where column `ci` / panel `k` (0 = GITHUB, 1 = AGENTS) was drawn this frame.
pub(crate) fn note_col(app: &App, ci: usize, r: Rect) {
    let mut c = app.col_rects.get();
    c[ci] = r;
    app.col_rects.set(c);
}

pub(crate) fn note_area(app: &App, k: usize, r: Rect) {
    if app.view != View::Board {
        return;
    }
    let mut a = app.area_rects.get();
    a[k] = Some(r);
    app.area_rects.set(a);
}

fn draw_column(f: &mut Frame, app: &App, ci: usize, area: Rect, plan: CardPlan) {
    note_col(app, ci, area);
    let col = COLUMNS[ci];
    let cards = app.col_cards(ci);
    let focused = ci == app.col;
    let n = cards.len();
    let count = if col == "doing" { format!("{n}/{}", app.snap.wip) } else { n.to_string() };
    // the header is ` o NAME (count) ` between the two corners: the count is never pushed off
    let room = (area.width as usize).saturating_sub(2 + 7 + count.len());
    let name = column_name(&app.snap, col, room);
    let note = date_order_note(&app.snap, col, room.saturating_sub(cells(&name)));
    let full = col == "doing" && n as i64 >= app.snap.wip;
    let colour = column_colour_in(col, &app.snap.theme);
    let hs = bold().fg(colour);
    let mut title = vec![
        Span::raw(" "),
        Span::styled("o", Style::default().fg(colour)),
        Span::styled(format!(" {name} ("), hs),
        Span::styled(count, if full { hs.add_modifier(Modifier::REVERSED) } else { hs }),
        Span::styled(") ", hs),
    ];
    if !note.is_empty() {
        title.push(Span::styled(note, dim()));
    }
    let title = Line::from(title);
    // Column frame: plain fg (cards carry the colour now; less busy), thick when focused.
    // Column frame in the column's colour (same as its cards), thick when focused.
    let block = frame(focused, Some(colour)).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let sel = if focused { Some(app.row[ci].min(n.saturating_sub(1))) } else { None };
    if cards.is_empty() {
        // first-run hint: a bare '-' told a new user nothing. It wraps at words to fit the
        // column; a column too small for that gets the short form, and one too small for
        // even that keeps the bare '-' — never a cut word.
        let hint = if ci == 0 && app.snap.cards.is_empty() {
            let (w, h) = ((inner.width as usize).saturating_sub(1), inner.height as usize);
            [FIRST_CARD_HINT, FIRST_CARD_SHORT].into_iter().find_map(|t| wrap_whole(t, w, h))
        } else {
            None
        };
        let lines: Vec<Line> = match hint {
            Some(wrapped) => wrapped.into_iter().map(|l| Line::styled(format!(" {l}"), dim())).collect(),
            None => vec![Line::styled(" -", dim())],
        };
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        match plan {
            CardPlan::Natural { .. } if inner.height < 3 || inner.width < 8 => {
                app.drawn_styles.borrow_mut().push((ci, "compact"));
                draw_compact(f, app, &cards, sel, inner);
            }
            CardPlan::Natural { dense } => {
                let used_dense = draw_boxed(f, app, &cards, sel, inner, colour, dense);
                app.drawn_styles.borrow_mut().push((ci, if used_dense { "dense" } else { "full" }));
            }
            CardPlan::Boxed => {
                app.drawn_styles.borrow_mut().push((ci, "boxed"));
                draw_fill(f, app, &cards, sel, inner, colour, true);
            }
            CardPlan::Line => {
                app.drawn_styles.borrow_mut().push((ci, "line"));
                draw_fill(f, app, &cards, sel, inner, colour, false);
            }
        }
    }
}

/// The `+N more` hint, in the longest form that fits `width` cells: `+10 more`, then `+10`,
/// then `+`. It shortens in WHOLE words like every other hint on the board — a cut `+10 mor`
/// reads like a defect, and this is the line that promises nothing is hidden silently.
pub fn more_hint(n: usize, width: usize) -> String {
    for form in [format!(" +{n} more"), format!(" +{n}"), format!("+{n}"), "+".to_string()] {
        if form.chars().count() <= width {
            return form;
        }
    }
    String::new()
}



/// Box heights (4-row style) of column `ci`'s cards in a column `width` wide; a dense box
/// is one row shorter.
pub(crate) fn card_box_heights(app: &App, ci: usize, width: u16) -> Vec<u16> {
    let text_w = width.saturating_sub(6) as usize; // column frame + card frame + padding
    app.col_cards(ci).iter().map(|c| card_lines(app, c, false, text_w, true).len() as u16 + 2).collect()
}

/// Rows column `ci` needs to show every card boxed (frame included; 3 when empty).
pub(crate) fn column_height(app: &App, ci: usize, width: u16, dense: bool) -> u16 {
    let h = card_box_heights(app, ci, width);
    if h.is_empty() {
        return 3;
    }
    2 + h.iter().map(|x| x - u16::from(dense)).sum::<u16>()
}

/// THE LAYOUT INVARIANT: every pane of the board — the four columns side by side, the two
/// rows of the 2x2 grid, the stacked sections — is the SAME size, give or take one cell.
/// `total` cells are split into `n` extents that differ by at most 1, whatever the panes
/// hold: a column of sixty finished cards gets exactly the room of a column of four. What
/// does not fit shows `+N more`, and the arrow keys scroll into it.
///
/// This is the ONE function that decides a pane's extent. Panes of CARDS stacked above each
/// other (the grid's two rows, the stacked sections) take its rule with no row of
/// difference at all (`layouts::equal_rows`), so equal boxes hold an equal number of cards. Sizing panes by what they hold
/// (the round-robin growth this replaced) let the fullest pile win the height — a TODO of
/// twenty squeezed to four cards while REVIEW sat half empty beside a long DONE — which is
/// not what a board is for: every column is read, so every column gets the same share.
///
/// Boundaries are placed at `round(i * total / n)` (halves round up), which is exactly what
/// `Constraint::Ratio(1, n)` gives, so a split made here and one made by ratatui agree cell
/// for cell and the side-by-side layouts render as they always have.
pub fn even_extents(total: u16, n: usize) -> Vec<u16> {
    if n == 0 {
        return Vec::new();
    }
    let (t, n32) = (u32::from(total), n as u32);
    let edge = |i: u32| ((2 * i * t + n32) / (2 * n32)) as u16;
    (0..n32).map(|i| edge(i + 1) - edge(i)).collect()
}

/// `area` split into `n` panes by `even_extents` — side by side when `across`, stacked
/// otherwise. Every layout sizes its panes through this (or `even_extents` directly).
pub(crate) fn split_even(area: Rect, n: usize, across: bool) -> Vec<Rect> {
    let total = if across { area.width } else { area.height };
    let mut at = 0u16;
    even_extents(total, n)
        .into_iter()
        .map(|len| {
            let r = if across {
                Rect { x: area.x + at, width: len, ..area }
            } else {
                Rect { y: area.y + at, height: len, ..area }
            };
            at += len;
            r
        })
        .collect()
}

/// Rows column `ci` needs to show its first two cards as dense boxes (+ a `+N more` row).
pub(crate) fn column_min_boxed(app: &App, ci: usize, width: u16) -> u16 {
    let h = card_box_heights(app, ci, width);
    if h.is_empty() {
        return 1;
    }
    2 + h.iter().take(2).map(|x| x - 1).sum::<u16>() + u16::from(h.len() > 2)
}

/// Below this many rows per column, cards fall back to the unboxed compact list.
pub const BOXED_MIN_ROWS: usize = 12;

/// How many cards a box `height` rows tall holds in the fixed forms, for `n` cards: `(boxed,
/// total)` — `boxed` 3-row boxes first, then one-line cards in the rows left, the last row
/// kept for `+N more` when not all `n` fit. A box FILLS its rows: there is no cap on how
/// many cards it shows (equal boxes are what keep one long column from crowding the others,
/// which a ten-card cap used to do — and it left empty rows above `+N more`).
pub fn fill_slots(height: usize, n: usize, boxed: bool) -> (usize, usize) {
    if height == 0 {
        return (0, 0);
    }
    let b = if boxed { height.saturating_sub(1) / 3 } else { 0 };
    let all = b + (height - 3 * b);
    let total = if n <= all { n } else { all - 1 };
    (b.min(total), total)
}

/// The last line of a box with cards out of sight: `+N more` below (and how many are above
/// when the box has scrolled), in the longest form that fits.
fn fill_hint(above: usize, below: usize, width: usize) -> String {
    let forms = match (above, below) {
        (0, b) => return more_hint(b, width),
        (a, 0) => vec![format!(" +{a} above"), format!(" ^{a}"), format!("^{a}")],
        (a, b) => vec![format!(" +{a} above · +{b} more"), format!(" ^{a} · +{b} more"), format!(" ^{a} +{b}"), format!("+{b}")],
    };
    forms.into_iter().find(|f| f.chars().count() <= width).unwrap_or_default()
}

/// A card as ONE line `width` wide: `#id title…`, and a red ` x` at the end when blocked.
fn line_card(app: &App, card: &Card, selected: bool, width: usize) -> Line<'static> {
    let mark = if card.blocked.is_some() { " x" } else { "" };
    let mut line = card_lines(app, card, selected, width.saturating_sub(mark.len()), false).swap_remove(0);
    if !mark.is_empty() {
        line.spans.push(Span::styled(mark, red()));
    }
    line
}

/// A box drawn in a fixed card form (`CardPlan::Boxed` / `Line`): `fill_slots` cards from a
/// window that follows the selection, each whole, and — when any are out of sight — one
/// `+N more` on the box's last row.
fn draw_fill(f: &mut Frame, app: &App, cards: &[&Card], sel: Option<usize>, inner: Rect, colour: Color, boxed: bool) {
    let n = cards.len();
    let (nb, total) = fill_slots(inner.height as usize, n, boxed);
    // the window follows the selection (it is the last card in view when scrolled down), and
    // never scrolls past the end, so a box with cards out of sight never has an empty row
    let t = sel.unwrap_or(0).min(n.saturating_sub(1));
    let start = t.saturating_sub(total.saturating_sub(1)).min(n.saturating_sub(total));
    // the 3-row boxes are a run that holds the selection: the one-line cards sit above and
    // below it, so the selected card is always drawn in full
    let run = if t < start + nb { start } else { t + 1 - nb };
    let text_w = inner.width.saturating_sub(4) as usize;
    let mut y = inner.y;
    for (k, c) in cards.iter().enumerate().skip(start).take(total) {
        let selected = sel == Some(k);
        if (run..run + nb).contains(&k) {
            let lines = card_lines(app, c, selected, text_w, true);
            let mut t = lines[0].clone().style(Style::default().fg(palette(&app.snap.theme).fg));
            t.spans.insert(0, Span::raw(" "));
            t.spans.push(Span::raw(" "));
            let b = frame(selected, Some(colour)).padding(Padding::horizontal(1)).title(t);
            let body: Vec<Line> = lines.get(1).cloned().into_iter().collect();
            f.render_widget(Paragraph::new(body).block(b), Rect { y, height: 3, ..inner });
            y += 3;
        } else {
            let l = line_card(app, c, selected, inner.width.saturating_sub(1) as usize);
            f.render_widget(Paragraph::new(l), Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(1), height: 1 });
            y += 1;
        }
    }
    let below = n - (start + total).min(n);
    if start > 0 || below > 0 {
        let hint = Line::styled(fill_hint(start, below, inner.width as usize), dim());
        f.render_widget(Paragraph::new(hint), Rect { y: inner.y + inner.height - 1, height: 1, ..inner });
    }
}

fn draw_compact(f: &mut Frame, app: &App, cards: &[&Card], sel: Option<usize>, inner: Rect) {
    let first = 0;
    let window = cards;
    let mut lines = Vec::new();
    let (mut sel_start, mut sel_end) = (0, 0);
    let mut ends = Vec::new();
    for (i, c) in window.iter().enumerate() {
        if sel == Some(first + i) {
            sel_start = lines.len();
        }
        lines.extend(card_lines(app, c, sel == Some(first + i), inner.width as usize, false));
        ends.push(lines.len());
        if sel == Some(first + i) {
            sel_end = lines.len();
        }
    }
    // Scroll to the end of the selection — but never PAST ITS FIRST LINE, which is the one
    // carrying `#id` and the title. A selected card taller than the rows it has used to
    // scroll to its last line, so a one-row column showed the bare meta (`  0m`) and the
    // card had no identity on screen at all: a card drawn is a card you can name.
    let offset = sel_end.saturating_sub(inner.height as usize).min(sel_start);
    // this list scrolls too, so it owes the same `+N more` a boxed column gives: a card
    // nobody can see, with nothing saying it is there, is the one thing that must not happen
    let last_row = offset + inner.height as usize;
    let shown = ends.iter().filter(|e| **e <= last_row).count();
    let hidden = cards.len() - (first + shown.max(usize::from(!window.is_empty())));
    if hidden > 0 && inner.height == 1 {
        // one row and something hidden: the row says so. A card fragment with nothing to say
        // the others exist is exactly what must not happen, and the header keeps the count.
        f.render_widget(Paragraph::new(Line::styled(more_hint(cards.len(), inner.width as usize), dim())), inner);
        return;
    }
    if hidden > 0 && inner.height >= 2 {
        let rows = inner.height as usize - 1;
        f.render_widget(
            Paragraph::new(lines).scroll((offset.min(sel_end.saturating_sub(rows)) as u16, 0)),
            Rect { height: rows as u16, ..inner },
        );
        f.render_widget(
            Paragraph::new(Line::styled(more_hint(hidden, inner.width as usize), dim())),
            Rect { y: inner.y + inner.height - 1, height: 1, ..inner },
        );
        return;
    }
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

/// Trello-style: each card in its own box in the column colour; the selected one thick.
/// The confirm line for approving your own REVIEW card (a solo person is not trapped).
/// The force prompt on your own REVIEW card: it names every rule `y` gets past.
fn approve_own(id: i64, skips: &[(&'static str, crate::store::Code)]) -> Mode {
    let rules: Vec<&str> = skips.iter().map(|(r, _)| *r).collect();
    Mode::Confirm {
        action: Confirm::ApproveOwn(id),
        prompt: format!("this is your work — close it anyway, skipping: {}? y/n (logged)", rules.join(", ")),
    }
}

/// Scrolls so the selection is visible, with dim `+N more` hints for hidden cards.
fn draw_boxed(f: &mut Frame, app: &App, cards: &[&Card], sel: Option<usize>, inner: Rect, colour: Color, dense: bool) -> bool {
    let text_w = inner.width.saturating_sub(4) as usize; // borders + 1 space padding each side
    let bodies: Vec<Vec<Line<'static>>> = cards
        .iter()
        .enumerate()
        .map(|(i, c)| card_lines(app, c, sel == Some(i), text_w, true))
        .collect();
    let avail = inner.height as usize;
    // the frame's plan decides dense for every box (`card_plan`): a box never picks its own
    let skip_first = usize::from(dense);
    let h = |i: usize| bodies[i].len() + 2 - skip_first;
    // first visible card: advance until the selected card (plus hint rows) fits
    let target = sel.unwrap_or(0);
    let mut start = 0;
    while start < target {
        let used: usize = (start..=target).map(h).sum();
        let hints = usize::from(start > 0) + usize::from(target + 1 < cards.len());
        if used + hints <= avail {
            break;
        }
        start += 1;
    }
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    if start > 0 {
        let hint = Line::styled(more_hint(start, inner.width as usize), dim());
        f.render_widget(Paragraph::new(hint), Rect { y, height: 1, ..inner });
        y += 1;
    }
    let mut i = start;
    let stop = cards.len();
    while i < stop {
        let more_after = cards.len() - i - 1;
        let reserve = u16::from(more_after > 0);
        let room = bottom.saturating_sub(y + reserve) as usize;
        let full = h(i);
        let body: Vec<Line> = if full <= room {
            bodies[i][skip_first..].to_vec()
        } else if room >= 3 - skip_first {
            bodies[i][..1 - skip_first].to_vec() // box too tall: title line only
        } else if room >= 1 && i == target {
            // not even a small box fits: bare title line
            f.render_widget(Paragraph::new(bodies[i][0].clone()), Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(1), height: 1 });
            i += 1;
            break;
        } else {
            break;
        };
        let rect = Rect { x: inner.x, y, width: inner.width, height: body.len() as u16 + 2 };
        let mut b = frame(sel == Some(i), Some(colour)).padding(Padding::horizontal(1));
        if dense {
            // title text stays plain fg (a border title would otherwise take the column colour)
            let mut t = bodies[i][0].clone().style(Style::default().fg(palette(&app.snap.theme).fg));
            t.spans.insert(0, Span::raw(" "));
            t.spans.push(Span::raw(" "));
            b = b.title(t);
        }
        f.render_widget(Paragraph::new(body).block(b), rect);
        y += rect.height;
        i += 1;
    }
    // a card that is not on screen ALWAYS has a `+N more` saying so — at the bottom if there
    // is a row for it, and otherwise in place of the last card drawn, because a hidden card
    // with nothing to say it is the one thing this must never do
    let left = cards.len() - i;
    if left > 0 {
        let hint = Line::styled(more_hint(left, inner.width as usize), dim());
        f.render_widget(Paragraph::new(hint), Rect { y: bottom - 1, height: 1, ..inner });
    }
    dense
}

/// `"note" 13m` fitted to `room` columns, or just the age when there is no note (or no room
/// for one). An agent that never writes notes still shows how long its card has been quiet.
pub fn activity(note: Option<&String>, age: &str, room: usize) -> String {
    let age_w = age.chars().count();
    match note {
        // quotes + one space + the age, and at least a few characters of the note
        Some(n) if !age.is_empty() && room >= age_w + 3 + 4 => format!("\"{}\" {age}", fit(n, room - age_w - 3)),
        Some(n) if age.is_empty() && room >= 2 + 4 => format!("\"{}\"", fit(n, room - 2)),
        _ if age_w <= room => age.to_string(),
        _ => String::new(),
    }
}

/// What the AGENTS panel says when nobody is on this board and herdr shows no agent pane.
pub(crate) fn agents_empty(app: &App) -> Line<'static> {
    let text = match &app.agents {
        AgentsState::Agents(_) => " no agent panes in herdr".to_string(),
        AgentsState::Pending => " checking herdr...".to_string(),
        AgentsState::Unavailable(msg) => format!(" {msg}"),
    };
    Line::styled(text, dim())
}

/// The line that closes the AGENTS panel: herdr agents that are nobody on this board. They
/// are counted, never described — tb does not read other boards. The words in brackets go
/// whole when the panel is too narrow for them.
pub(crate) fn elsewhere_line(n: usize, width: usize) -> Line<'static> {
    let short = format!(" +{n} elsewhere");
    let long = format!("{short} (not on this board)");
    Line::styled(if long.chars().count() <= width { long } else { short }, dim())
}

/// A panel with fewer rows than it has lines still ends in the count: its last row says how
/// many actors of this board are below it and how many agents are elsewhere. A focused panel
/// scrolls instead (every row can be reached), so it is left alone; so is a panel of one or
/// two rows, where the count would cost half of what it shows (its title or the header
/// already carries the numbers).
pub(crate) fn close_clipped(mut lines: Vec<Line<'static>>, app: &App, height: usize, width: usize) -> Vec<Line<'static>> {
    let r = app.roster();
    if app.focus == Focus::Agents || height < 3 || lines.len() <= height || r.total() == 0 {
        return lines;
    }
    let hidden = r.here.len().saturating_sub(height - 1);
    let n = r.elsewhere.len();
    // an idle agent that still holds a card is the panel's own "!" warning row (card #94):
    // one of those must never go quiet just because it fell below the fold. The rows kept
    // above are untouched (someone holding a card already sorts before someone holding
    // none, board order first) — this only makes sure the closing line says when one of the
    // rows it swallowed was a problem, the same way it already says how many were swallowed.
    let shown = (height - 1).min(r.here.len());
    let idle_hidden = r.here[shown..].iter().filter(|row| row.idle_holder()).count();
    let warn = if idle_hidden > 0 { format!(" · {idle_hidden} idle, holds card") } else { String::new() };
    // (at least one actor is hidden: the lines only outnumber the rows when the actors do)
    let texts = if n == 0 {
        vec![format!(" +{hidden} more here{warn}"), format!(" +{hidden} more here"), format!(" +{hidden} more{warn}"), format!(" +{hidden} more")]
    } else {
        vec![
            format!(" +{hidden} more here · +{n} elsewhere{warn}"),
            format!(" +{hidden} more here · +{n} elsewhere"),
            format!(" +{hidden} more · +{n} elsewhere{warn}"),
            format!(" +{hidden} more · +{n} elsewhere"),
            format!(" +{} more{warn}", hidden + n),
            format!(" +{} more", hidden + n),
        ]
    };
    let last = texts.last().cloned().unwrap_or_default();
    let text = texts.into_iter().find(|t| t.chars().count() <= width).unwrap_or(last);
    lines.truncate(height - 1);
    lines.push(Line::styled(text, if idle_hidden > 0 { red() } else { dim() }));
    lines
}

/// `last note #13 5m`: what an actor holding no card last did here (the age goes first
/// when there is no room for both).
pub(crate) fn last_seen(app: &App, row: &crate::roster::Row, room: usize) -> String {
    let Some(e) = row.last else { return String::new() };
    let what = format!("last {} #{}", e.kind, e.card_id);
    let full = format!("{what} {}", crate::store::fmt_age((app.snap.now - e.ts).max(0)));
    [full, what].into_iter().find(|t| t.chars().count() <= room).unwrap_or_default()
}

fn agents_panel(app: &App, width: usize) -> Vec<Line<'static>> {
    let r = app.roster();
    if r.total() == 0 {
        return vec![agents_empty(app)];
    }
    let mut lines: Vec<Line<'static>> = r.here.iter().map(|row| agent_row(app, row, width)).collect();
    if !r.elsewhere.is_empty() {
        lines.push(elsewhere_line(r.elsewhere.len(), width));
    }
    lines
}

/// One actor of this board: mark, name, harness and status (`-` without a herdr pane of
/// exactly that name), then the card it holds or reviews and what it last said about it.
/// A later "harness · model" per actor goes in the harness cell, from `Row`.
fn agent_row(app: &App, row: &crate::roster::Row, width: usize) -> Line<'static> {
    let holds = row.idle_holder();
    let (mark, st) = match (holds, row.live.is_some(), row.status()) {
        (true, _, _) => ("!", red()),
        // no live status: no mark, and nothing dimmed — the board says it holds the card
        (_, false, _) => (" ", Style::default()),
        (_, _, "working") => ("*", Style::default().fg(GREEN)),
        // a blocked agent is shown by its mark only; red is reserved for cards
        (_, _, "blocked") => ("x", bold()),
        (_, _, "idle" | "done") => ("-", dim()),
        _ => ("?", dim()),
    };
    // colour only the warning marks; the rest of the row stays monochrome
    let text_st = match (holds, row.status()) {
        (true, _) => bold(),
        (_, "working") => Style::default(),
        _ => st,
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(mark, st),
        Span::raw(" "),
        Span::styled(format!("{:<14} ", fit(&row.name, 14)), text_st.add_modifier(Modifier::BOLD)),
        Span::raw(format!("{:<7.7} ", row.harness())),
        Span::styled(format!("{:<8.8} ", row.status()), text_st),
    ];
    let used = |spans: &[Span]| spans.iter().map(|s| s.content.chars().count()).sum::<usize>();
    let Some(c) = row.card else {
        spans.push(Span::raw(format!("{:<5} ", "-")));
        let room = width.saturating_sub(used(&spans));
        // the pane's own job line is live data: it gives way to what the board knows
        let seen = last_seen(app, row, room);
        let job = row.live.and_then(|a| a.job.clone()).map(|j| format!("{j} · {seen}")).filter(|t| t.chars().count() <= room);
        spans.push(Span::styled(job.unwrap_or(seen), dim()));
        return Line::from(spans);
    };
    spans.push(Span::raw(format!("#{:<4} ", c.id)));
    if let Some(n) = crate::store::shown_ref(c) {
        spans.push(Span::raw(format!("gh#{n} ")));
    }
    let title = if row.role == Some(crate::roster::CardRole::Reviewer) { format!("review: {}", c.title) } else { c.title.clone() };
    spans.push(Span::raw(format!("{:<28} ", fit(&title, 28))));
    spans.push(Span::raw(format!("{:>5} ", crate::store::coarse_age(app.snap.now - c.column_since))));
    if holds {
        // the duration is shown whole or not at all: a panel too narrow for it
        // keeps the plain warning
        let flag = "! idle, holds card";
        let age = idle_hold_age(app, row);
        let fits = used(&spans) + flag.chars().count() + age.chars().count() <= width;
        spans.push(Span::styled(format!("{flag}{}", if fits { age.as_str() } else { "" }), st));
    } else {
        let age = app
            .snap
            .last_event_at
            .get(&c.id)
            .map(|ts| crate::store::fmt_age((app.snap.now - ts).max(0)))
            .unwrap_or_default();
        let text = activity(app.snap.last_note.get(&c.id), &age, width.saturating_sub(used(&spans)));
        spans.push(Span::styled(text, dim()));
    }
    Line::from(spans)
}

fn detail_strip(app: &App) -> Vec<Line<'static>> {
    let Some(c) = app.selected() else {
        return vec![Line::styled(" no card selected - press a to add one", dim())];
    };
    let mut first = vec![Span::raw(format!("> #{} ", c.id))];
    if let Some(n) = crate::store::shown_ref(c) {
        first.push(Span::raw(format!("gh#{n} ")));
    }
    first.push(Span::styled(c.title.clone(), Style::default().add_modifier(Modifier::BOLD)));
    let mut rest = vec![c.column.clone()];
    if let Some(t) = &c.tag {
        rest.push(t.clone());
    }
    rest.push(c.owner.clone().unwrap_or_else(|| "unowned".into()));
    if let Some(r) = &c.reviewer {
        rest.push(format!("review {r}"));
    }
    first.push(Span::styled(format!(" - {}", rest.join(" - ")), dim()));
    let events = app
        .snap
        .recent
        .get(&c.id)
        .map(|v| v.iter().map(event_line).collect::<Vec<_>>().join(" - "))
        .unwrap_or_default();
    vec![Line::from(first), Line::styled(format!("  {events}"), dim())]
}

fn hint_spans(hints: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (k, d) in hints {
        spans.push(Span::styled(format!(" {k}"), bold()));
        spans.push(Span::raw(format!(" {d} ")));
    }
    spans
}

fn hints_len(hints: &[(&str, &str)]) -> usize {
    hints.iter().map(|(k, d)| k.chars().count() + d.chars().count() + 3).sum()
}

fn footer(app: &App, width: u16) -> Line<'static> {
    let key = bold();
    match &app.mode {
        Mode::Add(buf) => Line::from(vec![
            Span::styled(" add: ", key),
            Span::raw(format!("{buf}_")),
            Span::styled("   enter: due date  esc cancel  (tip: 'admin: renew domain')", dim()),
        ]),
        Mode::AddDue { title, buf } => Line::from(vec![
            Span::styled(" due for ", key),
            Span::styled(format!("\"{}\"", fit(title, 24)), bold()),
            Span::styled(" (YYYY-MM-DD, empty for none): ", key),
            Span::raw(format!("{buf}_")),
            Span::styled("   enter save  esc cancel", dim()),
        ]),
        Mode::AddCheck { id, buf } => Line::from(vec![
            Span::styled(format!(" add check #{id}: "), key),
            Span::raw(format!("{buf}_")),
            Span::styled("   enter save  esc cancel", dim()),
        ]),
        Mode::Note { id, buf, .. } => Line::from(vec![
            Span::styled(format!(" note #{id}: "), key),
            Span::raw(format!("{buf}_")),
            Span::styled("   enter save  esc cancel", dim()),
        ]),
        Mode::SendBack { id, buf } => Line::from(vec![
            Span::styled(format!(" send #{id} back — why: "), key),
            Span::raw(format!("{buf}_")),
            Span::styled("   enter send back  esc cancel", dim()),
        ]),
        Mode::Confirm { prompt, .. } => Line::from(vec![Span::styled(format!(" {prompt}"), key)]),
        _ if app.focus != Focus::Columns && app.status.is_none() => {
            let what = if app.focus == Focus::Github { "open" } else { "details" };
            Line::from(hint_spans(&[("up/down", "select"), ("enter", what), ("tab", "next area"), ("esc", "back"), ("?", "help")]))
        }
        _ => {
            if let Some((msg, is_err)) = &app.status {
                let st = if *is_err { bold() } else { Style::default() };
                return Line::from(vec![Span::styled(format!(" {msg}"), st), Span::styled("  (esc clears)", dim())]);
            }
            // the essentials; `?` has the rest. The focus view keeps its own arrow axis
            // (left/right = card, up/down = column): say so here, so the help agrees.
            let focus_view = app.last_shape.get() == Shape::Focus && app.view == View::Board;
            if focus_view {
                // the focus view's own arrow axis, stated where the keys are used; it is what
                // this footer must never lose, so the extras are dropped first (a, enter, then
                // q) and the limit / pick-repo hints stay in `?`
                let mut hints = vec![("a", "add"), ("enter", "open"), ("arrows", "card/col"), ("shift+<>", "move"), ("?", "help"), ("q", "quit")];
                for drop in ["enter", "a", "q"] {
                    if hints_len(&hints) <= width as usize {
                        break;
                    }
                    hints.retain(|(k, _)| *k != drop);
                }
                return Line::from(hint_spans(&hints));
            }
            let mut hints = vec![("a", "add"), ("e", "edit"), ("x", "del"), ("enter", "open"), ("shift+arrows", "move")];
            if app.col == 1 {
                hints.push(("+/-", "limit"));
            }
            hints.push(("B", "boards"));
            if app.gh.repo.is_none() && app.show_github {
                hints.push(("R", "github: pick repo"));
            }
            hints.extend([("?", "help"), ("q", "quit")]);
            // degrade one hint at a time, least useful first, like the focus view above:
            // `x` is destructive and rare, `e` has an obvious alternative (open the card),
            // `R` costs 21 columns and the github panel already says how to pick, `+/-` and
            // `B` are occasional. `shift+arrows` (the only non-obvious core action) and `?`
            // (where every dropped hint is documented) are the floor and are never dropped.
            for drop in ["x", "e", "R", "+/-", "B", "enter", "q", "a"] {
                if hints_len(&hints) <= width as usize {
                    break;
                }
                hints.retain(|(k, _)| *k != drop);
            }
            Line::from(hint_spans(&hints))
        }
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_popup(f: &mut Frame, app: &App, d: &CardDetail, full_width: bool) {
    let c = &d.card;

    let mut meta = Vec::new();
    if let Some(t) = &c.tag {
        meta.push(t.clone());
    }
    meta.push(c.owner.clone().unwrap_or_else(|| "unowned".into()));
    if let Some(r) = &c.reviewer {
        meta.push(format!("review {r}"));
    }
    meta.push(format!("{} {}", c.column, fmt_age(app.snap.now - c.column_since)));
    let meta = format!(" {} ", meta.join(" - "));
    let hint = " up/down select  enter check  a add  d delete  n note  esc close ";
    // titles would inherit the border colour; keep the text itself plain fg
    let fg = palette(&app.snap.theme).fg;
    let block = frame(true, Some(column_colour_in(&c.column, &app.snap.theme)))
        .title(Span::styled(format!(" {} ", card_head(c)), bold().fg(fg)))
        .title(Line::styled(meta, Style::default().fg(fg)).right_aligned())
        .title_bottom(Line::styled(hint, bold().fg(fg)));
    let mut lines: Vec<Line> = Vec::new();
    if c.description.is_empty() {
        lines.push(Line::styled("(no description)", dim()));
    } else {
        for l in c.description.lines() {
            lines.push(Line::raw(l.to_string()));
        }
    }
    if !d.checklist.is_empty() {
        lines.push(Line::raw(""));
        let cur = app.cursor.min(d.checklist.len() - 1);
        for (k, i) in d.checklist.iter().enumerate() {
            let st = if k == cur { bold().add_modifier(Modifier::REVERSED) } else { Style::default() };
            let text = format!("[{}] {} {}", if i.done { "x" } else { " " }, i.idx, i.text);
            lines.push(Line::styled(text, st));
        }
    }
    if !d.links.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("links:", dim()));
        for l in &d.links {
            lines.push(Line::raw(format!("  {} {}: {}", l.idx, l.label, l.value)));
        }
    }
    if let Some(n) = c.gh_ref {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::raw("issue "),
            Span::raw(format!("gh#{n}")),
        ]));
    }
    lines.push(Line::raw(""));
    let wrap_extra = c.description.lines().filter(|l| l.chars().count() > 76).count();
    let want = (lines.len() + wrap_extra + d.events.len() + 2) as u16;
    let width = if full_width { f.area().width } else { 80 };
    let area = centered(f.area(), width, want.clamp(8, 24));
    f.render_widget(Clear, area);
    let base = base_style(app);
    f.render_widget(Block::default().style(base), area);
    let max_ev = (area.height as usize).saturating_sub(lines.len() + wrap_extra + 2).max(2);
    let skip = d.events.len().saturating_sub(max_ev);
    for e in d.events.iter().skip(skip) {
        lines.push(Line::styled(event_line(e), dim()));
    }
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

/// A panel title from `parts` joined by ` · ` that fits a box `width` wide: drop trailing
/// parts first, then shorten an `owner/repo` to `repo`; never cut inside a token.
pub(crate) fn fit_title(parts: &[&str], width: u16) -> String {
    let room = (width as usize).saturating_sub(4);
    let render = |p: &[String]| format!(" {} ", p.join(" · "));
    let mut p: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    loop {
        let t = render(&p);
        if t.chars().count() <= room + 2 || p.len() == 1 {
            break;
        }
        // shorten owner/repo before dropping it
        if p.len() == 2 {
            if let Some((_, name)) = p[1].split_once('/') {
                if !p[1].contains(' ') {
                    p[1] = name.to_string();
                    continue;
                }
            }
        }
        p.pop();
    }
    let t = render(&p);
    if t.chars().count() <= room + 2 {
        t
    } else {
        format!(" {} ", p[0])
    }
}

/// GitHub panel height: frame + tiles (4) + PR table + issue table + "+N more".
pub const GITHUB_ROWS: u16 = 14;
/// Below this panel height the tiles collapse into one summary line.
const GITHUB_TILES_MIN: u16 = 10;

fn fail_red(text: &str) -> Line<'static> {
    // colour only the word FAIL
    match text.find("FAIL") {
        Some(i) => Line::from(vec![
            Span::raw(text[..i].to_string()),
            Span::styled("FAIL", red()),
            Span::raw(text[i + 4..].to_string()),
        ]),
        None => Line::raw(text.to_string()),
    }
}

fn draw_github(f: &mut Frame, app: &App, area: Rect) {
    note_area(app, 0, area);
    use ratatui::widgets::{Cell, Row, Table};
    let gh = &app.gh;
    let focused = app.focus == Focus::Github;
    let sel_style = bold().add_modifier(Modifier::REVERSED);
    let Some(repo) = gh.repo.clone() else {
        // no repo yet: a 1-line panel that invites picking one
        let b = frame(focused, None).title(Span::styled(" GITHUB ", bold()));
        let inner = b.inner(area);
        f.render_widget(b, area);
        let st = if focused { sel_style } else { Style::default() };
        f.render_widget(Paragraph::new(Line::styled(" no repo — enter to pick one (or R)", st)), inner);
        return;
    };
    let synced = gh.snap.as_ref().map(|s| crate::store::fmt_clock(s.fetched_at)).unwrap_or_else(|| "never".into());
    let (suffix, is_red) = github::sync_suffix(gh.error.as_deref(), gh.fails);
    let sync_text = format!("synced {synced}{suffix}");
    let sync_style = if is_red { red() } else { bold() };
    let title = if focused && app.gh_sel == 0 {
        Span::styled(fit_title(&["GITHUB", &format!("repo: {repo}  (enter to change)"), &sync_text], area.width), sel_style)
    } else {
        Span::styled(fit_title(&["GITHUB", &repo, &sync_text], area.width), sync_style)
    };
    let block = frame(focused, None).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let now = app.snap.now;
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    let Some(s) = &gh.snap else {
        if gh.error.is_none() {
            f.render_widget(Paragraph::new(Line::styled(" fetching...", dim())), Rect { y, height: 1, ..inner });
        }
        return;
    };
    let fac = github::factory(s, &app.snap.cards, now);
    // tiles, or one summary line when the panel is short
    if inner.width < 100 && area.height >= GITHUB_TILES_MIN && bottom.saturating_sub(y) >= 6 {
        // narrow (half-v): the four tiles as a 2x2 grid, label in each border
        layouts::draw_dense_tiles(f, app, s, Rect { y, height: 6, ..inner });
        y += 6;
    } else if area.height >= GITHUB_TILES_MIN && bottom.saturating_sub(y) >= 4 {
        // the unlabelled tiles are drawn as ever; a full page's label is used only where it
        // fits whole (long, else terse), so it never cuts or pushes out anything else
        let tiles = github::tiles_as(s, &fac, now, github::PageLabel::None);
        let labelled = github::PageLabel::LABELLED.map(|l| github::tiles_as(s, &fac, now, l));
        let row = Rect { y, height: 4, ..inner };
        let cells = Layout::horizontal([Constraint::Ratio(1, 4); 4]).spacing(1).split(row);
        for (k, (title, value, line2)) in tiles.into_iter().enumerate() {
            let v_style = if value == "FAIL" { red() } else { bold() };
            // narrow tiles: "PULL REQUESTS" -> "PRS" so the value stays whole
            let iw = cells[k].width.saturating_sub(4) as usize;
            let short = title.len() + 2 + value.len() > iw && title == "PULL REQUESTS";
            let head = |t: &str, v: &str| t.chars().count() + 2 + v.chars().count() <= iw;
            // a labelled value beside the title the plain value gets, else beside "PRS"
            let mut pick = None;
            if title == "PULL REQUESTS" {
                let titles: &[&str] = if short { &["PRS"] } else { &["PULL REQUESTS", "PRS"] };
                pick = titles
                    .iter()
                    .flat_map(|t| labelled.iter().map(move |f| (t.to_string(), f[k].1.clone())))
                    .find(|(t, v)| *v != value && head(t, v));
            }
            let (title, value) = pick.unwrap_or_else(|| (if short { "PRS".to_string() } else { title }, value));
            let fitting = labelled.iter().map(|f| f[k].2.clone()).find(|l| *l != line2 && l.chars().count() <= iw);
            let line2 = fitting.unwrap_or(line2);
            // both lines through the dense tile's fit helper: a line with no shorter whole
            // form left (the quiet-repo empty state has none) ends in `…`, never mid-word
            let lines = vec![
                layouts::fit_line(vec![Span::styled(format!("{title}  "), bold()), Span::styled(value, v_style)], iw),
                layouts::fit_line(vec![Span::raw(line2)], iw),
            ];
            let b = frame(false, None).padding(Padding::horizontal(1));
            f.render_widget(Paragraph::new(lines).block(b), cells[k]);
        }
        y += 4;
    } else if y < bottom {
        // a full page's label only where the whole line fits (long, else terse); else the
        // line without labels, cut as ever
        let width = |segs: &[(String, bool)]| 1 + segs.iter().map(|(t, _)| t.chars().count()).sum::<usize>();
        let segs = github::PageLabel::LABELLED
            .iter()
            .map(|l| github::compact_summary_as(s, &fac, *l))
            .find(|segs| width(segs) <= inner.width as usize)
            .unwrap_or_else(|| github::compact_summary_as(s, &fac, github::PageLabel::None));
        let spans: Vec<Span> = std::iter::once(Span::raw(" "))
            .chain(segs.into_iter().map(|(t, r)| if r { Span::styled(t, red()) } else { Span::raw(t) }))
            .collect();
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { y, height: 1, ..inner });
        y += 1;
    }
    let avail = bottom.saturating_sub(y) as usize;
    if avail < 2 {
        return;
    }
    let w = inner.width.saturating_sub(1);
    // wide tables only while their TITLE column keeps >= 30 chars; else the tidy rows
    let (Some((pr_keep, pr_title)), Some((is_keep, is_title))) = (gh_table_plan(w, &PR_COLS), gh_table_plan(w, &ISSUE_COLS)) else {
        layouts::draw_tidy_list(f, app, s, Rect { y, height: bottom - y, ..inner }, focused);
        return;
    };
    if s.prs.is_empty() && s.issues.is_empty() {
        // first-run hint: a quiet repo says so instead of an empty table area
        f.render_widget(Paragraph::new(Line::styled(" no open issues or PRs", dim())), Rect { y, height: 1, ..inner });
        return;
    }
    let age = |ts: &str| github::age_of(ts, now).map(crate::store::coarse_age).unwrap_or_else(|| "?".into());
    // row budget: PRs get up to a third (min 1 if any), issues the rest (+1 for "+N more")
    let pr_rows = if s.prs.is_empty() { 0 } else { s.prs.len().min((avail.saturating_sub(2) / 3).max(1)) };
    let pr_h = if pr_rows > 0 { pr_rows + 1 } else { 0 };
    let issue_space = avail.saturating_sub(pr_h);
    // PR table: GH# 8 · TITLE flex · CI 5 · REVIEW 7 · AGE 5 · BRANCH / ISSUE 30
    if pr_h > 0 {
        let title_w = pr_title as usize;
        // selection: GitHub row i (1-based after the repo row) is PR i-1
        let sel_pr = if focused && app.gh_sel >= 1 && app.gh_sel <= s.prs.len() { Some(app.gh_sel - 1) } else { None };
        let off = sel_pr.map_or(0, |p| (p + 1).saturating_sub(pr_rows));
        let rows: Vec<Row> = s
            .prs
            .iter()
            .zip(&fac.pr_links)
            .enumerate()
            .skip(off)
            .take(pr_rows)
            .map(|(k, (p, link))| {
                let br = match link {
                    Some((i, who)) => format!("{} -> gh#{i} ({who})", p.head_ref),
                    None => p.head_ref.clone(),
                };
                let draft = if p.is_draft { "(draft) " } else { "" };
                let cells = vec![
                    Cell::from(format!("gh#{}", p.number)),
                    Cell::from(fit(&format!("{draft}{}", github::short_title(&p.title)), title_w)),
                    Cell::from(fail_red(&p.ci)),
                    Cell::from(p.review.clone()),
                    Cell::from(age(&p.created_at)),
                    Cell::from(fit(&br, 30)),
                ];
                let row = Row::new(keep_cells(cells, &pr_keep));
                if sel_pr == Some(k) { row.style(sel_style) } else { row }
            })
            .collect();
        let header = Row::new(keep_cells(["GH#", "TITLE", "CI", "REVIEW", "AGE", "BRANCH / ISSUE"].to_vec(), &pr_keep)).style(bold());
        let t = Table::new(rows, table_widths(&PR_COLS, &pr_keep)).header(header).column_spacing(1);
        f.render_widget(t, Rect { x: inner.x + 1, y, width: w, height: pr_h as u16 });
        y += pr_h as u16;
    }
    if issue_space < 2 || fac.issues.is_empty() {
        return;
    }
    let total = fac.issues.len();
    let mut shown = issue_space - 1; // header
    let more = total > shown;
    if more {
        shown = shown.saturating_sub(1);
    }
    // Issue table: ISSUE 6 · TITLE flex · STATE 15 · WHO 8 · AGE 5 · LABELS 16
    let title_w = is_title as usize;
    let first_issue = 1 + s.prs.len();
    let sel_is = if focused && app.gh_sel >= first_issue { Some(app.gh_sel - first_issue) } else { None };
    let off = sel_is.map_or(0, |i| (i + 1).saturating_sub(shown));
    let rows: Vec<Row> = fac
        .issues
        .iter()
        .enumerate()
        .skip(off)
        .take(shown)
        .map(|(k, r)| {
            let cells = vec![
                Cell::from(format!("gh#{}", r.number)),
                Cell::from(fit(&github::short_title(&r.title), title_w)),
                Cell::from(fail_red(&fit(&r.state, 15))),
                Cell::from(fit(&r.who, 8)),
                Cell::from(age(&r.created_at)),
                Cell::from(fit(&r.labels.join(","), 16)),
            ];
            let row = Row::new(keep_cells(cells, &is_keep));
            if sel_is == Some(k) { row.style(sel_style) } else { row }
        })
        .collect();
    let header = Row::new(keep_cells(["GH#", "TITLE", "STATE", "WHO", "AGE", "LABELS"].to_vec(), &is_keep)).style(bold());
    let h = (shown + 1) as u16;
    f.render_widget(
        Table::new(rows, table_widths(&ISSUE_COLS, &is_keep)).header(header).column_spacing(1),
        Rect { x: inner.x + 1, y, width: w, height: h },
    );
    y += h;
    if more && y < bottom {
        let line = Line::styled(format!("   +{} more issues · tb github for all", total - shown), dim());
        f.render_widget(Paragraph::new(line), Rect { y, height: 1, ..inner });
    }
}

/// The TITLE column of a wide GitHub table never shrinks below this; tidy rows instead.
pub const GH_TITLE_MIN: u16 = 30;
/// Wide-table columns (name, width; TITLE = 0 is the flexible one).
const PR_COLS: [(&str, u16); 6] = [("pr", 8), ("title", 0), ("ci", 5), ("review", 7), ("age", 5), ("branch", 30)];
const ISSUE_COLS: [(&str, u16); 6] = [("issue", 8), ("title", 0), ("state", 15), ("who", 8), ("age", 5), ("labels", 16)];
/// Optional columns, dropped in this order before the title goes below `GH_TITLE_MIN`.
const GH_DROP: [&str; 5] = ["labels", "branch", "age", "who", "review"];

/// Which columns of a wide table fit in `w` with a title of at least `GH_TITLE_MIN`, and
/// the title width; None when even the bare table (number, title, status) can't.
fn gh_table_plan(w: u16, cols: &[(&str, u16); 6]) -> Option<([bool; 6], u16)> {
    let mut keep = [true; 6];
    let title = |keep: &[bool; 6]| {
        let n = keep.iter().filter(|k| **k).count() as u16;
        let fixed: u16 = cols.iter().zip(keep).filter(|(_, k)| **k).map(|(c, _)| c.1).sum();
        w.saturating_sub(fixed + n.saturating_sub(1))
    };
    let mut drops = GH_DROP.iter();
    while title(&keep) < GH_TITLE_MIN {
        let name = drops.next()?;
        if let Some(i) = cols.iter().position(|c| c.0 == *name) {
            keep[i] = false;
        }
    }
    Some((keep, title(&keep)))
}

fn keep_cells<T>(cells: Vec<T>, keep: &[bool; 6]) -> Vec<T> {
    cells.into_iter().zip(keep).filter(|(_, k)| **k).map(|(c, _)| c).collect()
}

fn table_widths(cols: &[(&str, u16); 6], keep: &[bool; 6]) -> Vec<Constraint> {
    let all = cols.iter().map(|c| if c.1 == 0 { Constraint::Min(8) } else { Constraint::Length(c.1) }).collect();
    keep_cells(all, keep)
}

/// Rows the full GitHub panel wants at `width` (frame, tiles, every PR/issue row).
pub(crate) fn github_want(app: &App, width: u16) -> u16 {
    let Some(s) = app.gh.snap.as_ref().filter(|_| app.gh.repo.is_some()) else {
        return 3;
    };
    let inner = width.saturating_sub(2);
    let tiles = if inner < 100 { 6 } else { 4 };
    let w = inner.saturating_sub(1);
    let wide = gh_table_plan(w, &PR_COLS).is_some() && gh_table_plan(w, &ISSUE_COLS).is_some();
    let issues = github::factory(s, &app.snap.cards, app.snap.now).issues.len() as u16;
    let rows = if wide {
        let prs = s.prs.len() as u16;
        (if prs > 0 { prs + 1 } else { 0 }) + (if issues > 0 { issues + 1 } else { 0 })
    } else {
        s.prs.len() as u16 + issues
    };
    2 + tiles + rows.max(1)
}

fn base_style(app: &App) -> Style {
    let p = palette(&app.snap.theme);
    Style::default().fg(p.fg).bg(p.bg)
}


/// How long an idle card-holder's card has been quiet (` (1h20m)`), from the card's last event.
pub(crate) fn idle_hold_age(app: &App, row: &crate::roster::Row) -> String {
    row.card
        .filter(|_| row.idle_holder())
        .and_then(|c| app.snap.last_event_at.get(&c.id))
        .map(|ts| format!(" ({})", crate::store::fmt_age((app.snap.now - ts).max(0))))
        .unwrap_or_default()
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    // paint every cell with the theme's fg/bg so the terminal theme never shows through
    f.render_widget(Block::default().style(base_style(app)), area);
    let shape = pick_shape(&app.snap.layout, area.width, area.height);
    if app.view != View::Board {
        layouts::draw_view(f, app, area);
    } else {
        app.col_rects.set([Rect::default(); 4]);
        app.area_rects.set([None, None]);
        app.drawn_styles.borrow_mut().clear();
        app.last_shape.set(shape);
        match shape {
            Shape::HalfH => draw_board(f, app, area, false),
            Shape::HalfV => layouts::draw_grid(f, app, area),
            Shape::ThirdV => layouts::draw_stack(f, app, area),
            Shape::ThirdH => layouts::draw_rail(f, app, area),
            Shape::Focus => layouts::draw_focus(f, app, area),
        }
    }
    if let (Mode::Popup(_) | Mode::AddCheck { .. } | Mode::Note { from_popup: true, .. }, Some(d)) =
        (&app.mode, &app.popup)
    {
        draw_popup(f, app, d, shape == Shape::ThirdV || shape == Shape::Focus);
    }
    if let Mode::Picker { filter, sel } = &app.mode {
        draw_picker(f, app, filter, *sel);
    }
    if let Mode::Boards { sel } = app.mode {
        draw_boards(f, app, sel);
    }
    if let Mode::GhItem { pr, number } = app.mode {
        draw_gh_item(f, app, pr, number);
    }
    if let Mode::AgentInfo(i) = app.mode {
        draw_agent_info(f, app, i);
    }
    match &app.mode {
        Mode::Edit(form) => draw_edit(f, app, form),
        Mode::Help => draw_help(f, app),
        _ => {}
    }
    // every cell of every view: displayed text never carries control characters or sequences
    crate::text::sanitize_buffer(f.buffer_mut());
}

/// Text with a reversed cursor cell at char index `cursor` (a space when at the end).
fn cursor_line(text: &str, cursor: usize, active: bool) -> Line<'static> {
    if !active {
        return Line::raw(text.to_string());
    }
    let chars: Vec<char> = text.chars().collect();
    let cur = cursor.min(chars.len());
    let before: String = chars[..cur].iter().collect();
    let at: String = chars.get(cur).map(|c| c.to_string()).unwrap_or_else(|| " ".into());
    let after: String = chars.get(cur + 1..).map(|s| s.iter().collect()).unwrap_or_default();
    Line::from(vec![Span::raw(before), Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)), Span::raw(after)])
}

/// The `e` edit form: Title (one line) and Description (one logical line that wraps).
/// The form's three fields, in tab order. Each wants a label row and a three-row box.
const FORM_ROWS_PER_FIELD: u16 = 4;

/// Which fields fit in `rows` of interior, given which one is being typed into.
///
/// Fields are dropped from the END — the description first — because the ones above it are
/// short and the description is the one that can be read on the card instead. The field
/// being typed into is ALWAYS drawn, whatever else goes: typing into something invisible is
/// worse than a missing box. Returns the field indexes to draw, in order.
pub fn form_fields_for(rows: u16, active: u8) -> Vec<u8> {
    let fits = (rows / FORM_ROWS_PER_FIELD).min(FORM_FIELDS as u16) as usize;
    let mut shown: Vec<u8> = (0..fits as u8).collect();
    if fits > 0 && !shown.contains(&active) {
        // the last one makes way for the field the cursor is in
        let last = shown.len() - 1;
        shown[last] = active;
    }
    shown
}

fn draw_edit(f: &mut Frame, app: &App, form: &EditForm) {
    // three fields (title, due, description) want 12 rows of interior plus the border
    let want = if f.area().height >= 18 { 18 } else { 14 };
    let area = centered(f.area(), 80, want);
    // below this even one field cannot be drawn whole; the form stays open, and `esc` and
    // `enter` still work, so nothing is lost by drawing nothing here
    if area.width < 10 || area.height < FORM_ROWS_PER_FIELD + 2 {
        return;
    }
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(base_style(app)), area);
    let b = frame(true, None)
        .title(Span::styled(format!(" Edit #{} ", form.id), bold()))
        .title_bottom(Line::styled(" tab field  left/right home/end move  enter save  esc cancel ", bold()));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let pad = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let label = |active: bool, t: &str| Line::styled(t.to_string(), if active { bold() } else { dim() });
    // one field: a label row, then a boxed line of text with the cursor in it. Every row is
    // worked out from the height the form actually got, never from a fixed offset.
    let mut field = |y: u16, height: u16, active: bool, name: &str, text: &str, wrap: bool| {
        if y + 1 + height > pad.height {
            return; // never draw past the box: a short pane drops a field, it does not panic
        }
        f.render_widget(Paragraph::new(label(active, name)), Rect { y: pad.y + y, height: 1, ..pad });
        let block = frame(active, None).padding(Padding::horizontal(1));
        let rect = Rect { y: pad.y + y + 1, height, ..pad };
        // keep the cursor visible in a long line
        let w = rect.width.saturating_sub(4) as usize;
        let skip = if active && !wrap { form.cursor.saturating_sub(w.saturating_sub(1)) } else { 0 };
        let shown: String = text.chars().skip(skip).collect();
        let line = cursor_line(&shown, form.cursor - skip.min(form.cursor), active);
        let p = Paragraph::new(line).block(block);
        f.render_widget(if wrap { p.wrap(Wrap { trim: false }) } else { p }, rect);
    };
    let shown = form_fields_for(pad.height, form.field);
    let last = shown.len().saturating_sub(1);
    for (row, which) in shown.iter().enumerate() {
        let y = row as u16 * FORM_ROWS_PER_FIELD;
        // the last field drawn takes the rest of the box (the description wraps into it)
        let height = if row == last { pad.height.saturating_sub(y + 1) } else { 3 };
        match which {
            0 => field(y, height.min(3), form.field == 0, "Title  (tag: prefix sets the tag)", &form.title, false),
            1 => field(y, height.min(3), form.field == 1, "Due  (YYYY-MM-DD, empty for none)", &form.due, false),
            _ => field(y, height, form.field == 2, "Description", &form.desc, true),
        }
    }
    // a pane too short for every field says which are not on screen, rather than hiding them
    let hidden: Vec<&str> = [(0u8, "title"), (1, "due"), (2, "description")]
        .iter()
        .filter(|(i, _)| !shown.contains(i))
        .map(|(_, n)| *n)
        .collect();
    let used = shown.len() as u16 * FORM_ROWS_PER_FIELD;
    // a dropped field is never silent, at ANY height: when there is a free row below the
    // last field, the notice takes it (unchanged from before); when there is not — every
    // field slot is already at its own minimum, so there is no spare row to reserve without
    // shrinking a box below the 3 rows a border+text+border needs — the notice instead takes
    // the LAST row on screen, over the bottom border of the last field shown. That is the
    // same trade the card column already makes for its own `+N more` hint (`draw_boxed`):
    // one row of a box's border is a smaller loss than a field nobody is told about (card
    // #107 — `used < pad.height` skipped the notice at exactly the heights, 8 and 12, where
    // `used == pad.height` and no free row exists; the sole existing test only checked h=14,
    // where a free row happens to exist, so the gap went uncaught).
    if !hidden.is_empty() {
        let text = format!("{} not shown — make the pane taller (tab still reaches it)", hidden.join(" and "));
        let y = used.min(pad.height.saturating_sub(1));
        f.render_widget(Paragraph::new(Line::styled(fit(&text, pad.width as usize), dim())), Rect { y: pad.y + y, height: 1, ..pad });
    }
}

/// Every key, grouped, plus the CLI verbs.
pub const HELP_GROUPS: [(&str, &[(&str, &str)]); 6] = [
    ("Board", &[
        ("arrows", "select a card (left/right column, up/down card)"),
        ("focus view arrows", "left/right card, up/down column"),
        ("shift+left/right", "move the card to the next column (also > <)"),
        ("shift+up/down, K J", "reorder the card within its column"),
        ("tab / shift+tab", "cycle focus: columns, GITHUB, AGENTS"),
        ("q", "quit"),
    ]),
    ("Cards", &[
        ("a", "add a card ('tag: title'), then an optional due date"),
        ("e", "edit title and description"),
        ("x", "delete (asks y/n)"),
        ("enter", "open the card"),
        ("d", "done: doing -> review, review/todo -> done"),
        ("n", "add a note"),
        ("+ / -", "raise / lower the WIP limit (DOING focused)"),
    ]),
    ("Card popup", &[
        ("up/down, enter", "select / toggle a checklist item"),
        ("a / d", "add / delete a checklist item"),
        ("n / e", "note / edit"),
        ("esc", "close"),
    ]),
    ("Panels", &[
        ("down from the last card", "enter GITHUB, then AGENTS"),
        ("up/down, enter", "select, open (repo row -> picker; PR/issue -> popup)"),
        ("a / o", "in a PR/issue popup: add to board / open in browser"),
        ("esc", "back to the columns"),
    ]),
    ("View", &[
        ("T", "dark / light theme"),
        ("L", "view: auto, focus, third-h, third-v, half-h, half-v"),
        ("A / G", "show / hide AGENTS / GITHUB"),
        ("B", "boards: switch without quitting; * default, a archive, r restore, d delete"),
        ("R", "pick the GitHub repo"),
        ("?", "this help"),
    ]),
    ("CLI", &[
        ("cards", "add list show edit rm note check block"),
        ("flow", "next take done move drop prio"),
        ("board", "boards config github sync agents guide"),
        ("apps", "board --json · watch --json · every write takes --json"),
    ]),
];

/// The empty-TODO hint on a board with no cards, and its form for tiny columns.
pub const FIRST_CARD_HINT: &str = "press a to add your first card";
pub const FIRST_CARD_SHORT: &str = "a: add a card";

/// `text` wrapped at spaces into at most `height` lines of `width`, or None when that would
/// split a word or need more lines.
fn wrap_whole(text: &str, width: usize, height: usize) -> Option<Vec<String>> {
    if text.split_whitespace().any(|word| word.chars().count() > width) {
        return None;
    }
    let lines = wrap_words(text, width);
    (lines.len() <= height).then_some(lines)
}

/// Split `text` into lines of at most `width` characters, at spaces (a longer word is cut).
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = cur.chars().count();
            let sep = usize::from(used > 0);
            if used + sep + word.len() <= width {
                if sep == 1 {
                    cur.push(' ');
                }
                cur.extend(word.iter());
                break;
            }
            if used > 0 {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            // a word longer than the line: cut it
            let rest = word.split_off(width);
            out.push(word.into_iter().collect());
            word = rest;
            if word.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// The help rows for an overlay `iw` columns wide: keys in a `kw` column, descriptions
/// wrapped beside them; a key too long for the column gets its own line.
fn help_lines(iw: usize, kw: usize) -> Vec<Line<'static>> {
    let indent = 3 + kw;
    let dw = iw.saturating_sub(indent).max(8);
    let mut lines = Vec::new();
    for (group, keys) in HELP_GROUPS {
        lines.push(Line::styled(format!(" {group}"), bold()));
        for (k, d) in keys {
            let mut desc = wrap_words(d, dw).into_iter();
            if k.chars().count() < kw {
                let first = desc.next().unwrap_or_default();
                lines.push(Line::from(vec![Span::styled(format!("   {k:<kw$}"), bold()), Span::raw(first)]));
            } else {
                lines.push(Line::styled(format!("   {k}"), bold()));
            }
            for more in desc {
                lines.push(Line::raw(format!("{:indent$}{more}", "")));
            }
        }
    }
    lines
}

fn draw_help(f: &mut Frame, app: &App) {
    // narrow panes: a smaller overlay, a shrunk key column, descriptions wrapped
    let wide = f.area().width >= 100;
    let (w, kw) = if wide { (96, 26) } else { (f.area().width.saturating_sub(2).max(30), 12) };
    let iw = w.min(f.area().width.saturating_sub(2)).saturating_sub(2) as usize;
    let lines = help_lines(iw, kw);
    let rows = lines.len();
    let area = centered(f.area(), w, rows as u16 + 3);
    if area.width < 10 || area.height < 4 {
        return;
    }
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(base_style(app)), area);
    let mut title_bottom = Line::styled(" esc or ? closes ", bold());
    let inner_h = area.height.saturating_sub(2) as usize;
    if rows > inner_h {
        title_bottom = Line::styled(" esc/? closes · up/down scroll ", bold());
    }
    let b = frame(true, None)
        .title(Span::styled(" Terminal Board keys ", bold()))
        .title_bottom(title_bottom);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let max_scroll = rows.saturating_sub(inner_h) as u16;
    app.help_max.set(max_scroll);
    let scroll = app.help_scroll.min(max_scroll);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn info_popup(f: &mut Frame, app: &App, title: String, lines: Vec<Line<'static>>, hint: &str) {
    let h = (lines.len() as u16 + 2).max(5);
    let area = centered(f.area(), 90, h);
    if area.width < 4 || area.height < 3 {
        return;
    }
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(base_style(app)), area);
    let b = frame(true, None)
        .title(Span::styled(title, bold()))
        .title_bottom(Line::styled(hint.to_string(), bold()));
    f.render_widget(Paragraph::new(lines).block(b).wrap(Wrap { trim: false }), area);
}

/// Enter on a PR/issue row in the GITHUB panel.
fn draw_gh_item(f: &mut Frame, app: &App, pr: bool, number: i64) {
    let Some(s) = &app.gh.snap else { return };
    let now = app.snap.now;
    let age = |ts: &str| github::age_of(ts, now).map(fmt_age).unwrap_or_else(|| "?".into());
    let fac = github::factory(s, &app.snap.cards, now);
    let kind = if pr { "pull" } else { "issues" };
    let mut lines = Vec::new();
    if pr {
        let Some((p, link)) = s.prs.iter().zip(&fac.pr_links).find(|(p, _)| p.number == number) else { return };
        lines.push(Line::styled(p.title.clone(), bold()));
        lines.push(fail_red(&format!("CI {} · review {} · by {} · {} old", p.ci, p.review, p.author, age(&p.created_at))));
        let link = link.as_ref().map(|(i, w)| format!(" -> #{i} ({w})")).unwrap_or_default();
        lines.push(Line::raw(format!("branch {}{link}{}", p.head_ref, if p.is_draft { " · draft" } else { "" })));
    } else {
        let Some(r) = fac.issues.iter().find(|r| r.number == number) else { return };
        lines.push(Line::styled(r.title.clone(), bold()));
        lines.push(fail_red(&format!("state {} · who {} · {} old", r.state, r.who, age(&r.created_at))));
        let labels = if r.labels.is_empty() { "-".to_string() } else { r.labels.join(", ") };
        lines.push(Line::raw(format!("labels {labels}")));
    }
    if let Some(c) = app.snap.cards.iter().find(|c| c.gh_ref == Some(number)) {
        lines.push(Line::raw(format!("on the board as #{} ({})", c.id, c.column)));
    }
    lines.push(Line::styled(format!("https://github.com/{}/{kind}/{number}", s.repo), dim()));
    let what = if pr { "PR" } else { "issue" };
    info_popup(f, app, format!(" {what} #{number} "), lines, " a add to board  o open in browser  esc close ");
}

/// Enter on a row of the AGENTS panel: one actor of this board, or the `+N elsewhere` line.
fn draw_agent_info(f: &mut Frame, app: &App, i: usize) {
    let r = app.roster();
    let Some(row) = r.here.get(i) else {
        if i == r.here.len() && !r.elsewhere.is_empty() {
            // named, never described: tb does not read the boards they work on
            let mut lines = vec![Line::styled("herdr agents that hold or review nothing on this board", dim())];
            lines.extend(r.elsewhere.iter().map(|a| Line::raw(format!("{:<16} {:<8} {}", fit(&a.name, 16), a.harness, a.status))));
            info_popup(f, app, format!(" {} elsewhere ", r.elsewhere.len()), lines, " esc close ");
        }
        return;
    };
    let mut lines = match row.live {
        Some(a) => vec![
            Line::raw(format!("harness {} · status {} · pane {}", a.harness, a.status, a.pane_id)),
            Line::raw(format!("job {}", a.job.clone().unwrap_or_else(|| "-".into()))),
        ],
        None => vec![Line::styled("no live status: no herdr agent has exactly this name", dim())],
    };
    let hint = match (row.card, row.role) {
        (Some(c), role) => {
            let verb = if role == Some(crate::roster::CardRole::Reviewer) { "reviews" } else { "holds" };
            lines.push(Line::raw(format!("{verb} #{} {} ({})", c.id, c.title, c.column)));
            " enter jump to card  esc close "
        }
        _ => {
            lines.push(Line::styled(format!("holds no card · {}", last_seen(app, row, usize::MAX)), dim()));
            " esc close "
        }
    };
    info_popup(f, app, format!(" {} ", row.name), lines, hint);
}

/// The `R` repo picker popup: search box, `off` row, owner-grouped repo table.
fn draw_picker(f: &mut Frame, app: &App, filter: &str, sel: usize) {
    use ratatui::widgets::{Cell, Row, Table};
    let screen = f.area();
    let groups = app.picker_groups(filter);
    let n_repos: usize = groups.iter().map(|g| g.1.len()).sum();
    let total = match &app.repos {
        RepoState::Loaded(v) => v.len(),
        _ => 0,
    };
    let typed = filter.trim();
    let fallback = n_repos == 0 && github::valid_repo(typed);
    // display rows: None = group header (owner), Some(k) = selectable repo k (1-based picker row)
    let mut disp: Vec<(Option<usize>, String, Option<&github::RepoEntry>)> = Vec::new();
    let mut k = 0;
    for (owner, repos) in &groups {
        disp.push((None, owner.clone(), None));
        for r in repos {
            k += 1;
            disp.push((Some(k), r.name_with_owner.split('/').nth(1).unwrap_or("").to_string(), Some(*r)));
        }
    }
    let list = disp.len().max(1) + usize::from(fallback);
    let w = 100.min(screen.width.saturating_sub(4));
    let h = (list as u16 + 10).min(screen.height.saturating_sub(2));
    let area = Rect { x: screen.x + (screen.width - w) / 2, y: screen.y + (screen.height - h) / 2, width: w, height: h };
    if w < 10 || h < 5 {
        return;
    }
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(base_style(app)), area);
    let b = frame(true, None)
        .title(Span::styled(" Pick a GitHub repo ", bold()))
        .title_bottom(Line::styled(" type to search  up/down select  enter pick  esc cancel ", bold()));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let pad = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    // search box
    if bottom.saturating_sub(y) >= 3 {
        let sb = frame(false, None).padding(Padding::horizontal(1));
        let line = Line::from(vec![Span::styled("search: ", bold()), Span::raw(format!("{filter}_"))]);
        f.render_widget(Paragraph::new(line).block(sb).style(Style::default()), Rect { y, height: 3, ..pad });
        y += 3;
    }
    let sel_st = bold().add_modifier(Modifier::REVERSED);
    // off row
    if y < bottom {
        let st = if sel == 0 { sel_st } else { dim() };
        let text = format!("{}  off   turn GitHub off", if sel == 0 { ">" } else { " " });
        f.render_widget(Paragraph::new(Line::styled(fit(&text, pad.width as usize), st)), Rect { y, height: 1, ..pad });
        y += 2;
    }
    // footer count (last inner row)
    let count_y = bottom.saturating_sub(1);
    if count_y > y {
        let mut msg = format!("{n_repos} of {total} repos · * = current");
        if let Some(m) = &app.picker_msg {
            msg = m.clone();
        }
        let st = if app.picker_msg.is_some() { red() } else { dim() };
        f.render_widget(Paragraph::new(Line::styled(fit(&msg, pad.width as usize), st)), Rect { y: count_y, height: 1, ..pad });
    }
    let table_area = Rect { y, height: count_y.saturating_sub(y + 1), ..pad };
    if table_area.height == 0 {
        return;
    }
    let centred = |f: &mut Frame, text: &str, st: Style| {
        let r = Rect { y: table_area.y + table_area.height / 2, height: 1, ..table_area };
        f.render_widget(Paragraph::new(Line::styled(text.to_string(), st)).alignment(ratatui::layout::Alignment::Center), r);
    };
    match &app.repos {
        RepoState::Loading | RepoState::Idle => return centred(f, "loading repos…", dim()),
        RepoState::Error(e) => return centred(f, e, red()),
        RepoState::Loaded(_) => {}
    }
    // columns: marker 2 · REPO (min 20) · visibility 9 · PUSHED 7 · DESCRIPTION (rest)
    let longest = disp.iter().map(|d| d.1.chars().count() + 2).max().unwrap_or(0);
    let repo_w = (longest as u16 + 2).clamp(20, 32);
    let desc_w = table_area.width.saturating_sub(2 + repo_w + 9 + 7 + 4) as usize;
    let cur = app.gh.repo.clone();
    let mut rows: Vec<Row> = Vec::new();
    let mut sel_disp = 0;
    for (i, (key, name, repo)) in disp.iter().enumerate() {
        match (key, repo) {
            (Some(k), Some(r)) => {
                let selected = *k == sel;
                if selected {
                    sel_disp = i;
                }
                let current = cur.as_deref() == Some(r.name_with_owner.as_str());
                let marker = format!("{}{}", if selected { ">" } else { " " }, if current { "*" } else { " " });
                let pushed = github::age_of(&r.pushed_at, app.snap.now).map(crate::store::coarse_age).unwrap_or_default();
                let row = Row::new(vec![
                    Cell::from(marker),
                    Cell::from(fit(&format!("  {name}"), repo_w as usize)),
                    Cell::from(if r.is_private { "private" } else { "" }),
                    Cell::from(pushed),
                    Cell::from(Line::styled(fit(&r.description, desc_w), dim())),
                ]);
                rows.push(if selected { row.style(sel_st) } else { row });
            }
            _ => rows.push(Row::new(vec![Cell::from(""), Cell::from(Line::styled(name.clone(), dim()))])),
        }
    }
    if fallback {
        // nothing matches, but it looks like owner/repo: one full-width row under the header
        let selected = sel == 1;
        let text = fit(&format!("{} use {typed} (enter to check)", if selected { ">" } else { " " }), table_area.width as usize);
        let st = if selected { sel_st } else { Style::default() };
        let header = Row::new(["", "REPO", "", "PUSHED", "DESCRIPTION"]).style(bold());
        let widths = [Constraint::Length(2), Constraint::Length(repo_w), Constraint::Length(9), Constraint::Length(7), Constraint::Min(0)];
        f.render_widget(Table::new(Vec::<Row>::new(), widths).header(header).column_spacing(1), table_area);
        if table_area.height > 1 {
            f.render_widget(Paragraph::new(Line::styled(text, st)), Rect { y: table_area.y + 1, height: 1, ..table_area });
        }
        return;
    }
    let body_h = table_area.height.saturating_sub(1) as usize; // minus the header row
    let off = (sel_disp + 1).saturating_sub(body_h);
    let rows: Vec<Row> = rows.into_iter().skip(off).collect();
    let widths = [
        Constraint::Length(2),
        Constraint::Length(repo_w),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Min(0),
    ];
    let header = Row::new(["", "REPO", "", "PUSHED", "DESCRIPTION"]).style(bold());
    f.render_widget(Table::new(rows, widths).header(header).column_spacing(1), table_area);
}

/// The board picker's count columns (header, width), in `COLUMNS` order. A column that does
/// not fit is dropped whole, right to left, so a header is never cut in half.
const BOARD_COLS: [(&str, u16); 4] = [("TODO", 4), ("DOING", 5), ("REVIEW", 6), ("DONE", 4)];
/// The name column is never narrower than its own header.
const BOARD_NAME_HEAD: &str = "BOARD";
/// The overlay's bottom hint, longest form first; the widest one that fits is used.
const BOARD_HINTS: [&str; 4] = [
    " up/down select · enter switch · * default · a archive · r restore · d delete · esc cancel ",
    " enter switch · * default · a archive · r restore · d delete · esc ",
    " enter switch · esc cancel ",
    " esc ",
];
/// The line between the live boards and the archived ones.
const BOARD_ARCHIVED_HEAD: &str = "archived";

/// Which count columns fit in `w` cells, and how wide the name column is then. `longest` is
/// the longest board name. Widths are the marker (2), the name, each kept count column, and
/// one cell of spacing between them.
fn boards_plan(w: u16, longest: u16) -> ([bool; 4], u16) {
    let mut keep = [true; 4];
    let want = longest.max(BOARD_NAME_HEAD.len() as u16);
    let room = |keep: &[bool; 4]| {
        let counts: u16 = BOARD_COLS.iter().zip(keep).filter(|(_, k)| **k).map(|(c, _)| c.1 + 1).sum();
        w.saturating_sub(3 + counts)
    };
    for i in (0..4).rev() {
        if room(&keep) >= want {
            break;
        }
        keep[i] = false;
    }
    (keep, room(&keep).min(want).max(1))
}

/// Width the overlay wants: every count column, the longest name, and room for the hint.
fn boards_width(longest: u16) -> u16 {
    let counts: u16 = BOARD_COLS.iter().map(|c| c.1 + 1).sum();
    let content = 3 + longest.max(BOARD_NAME_HEAD.len() as u16) + counts + 4; // + frame + padding
    content.max(BOARD_HINTS[0].chars().count() as u16 + 2)
}

/// The `B` board picker: the rows of `tb boards` — name, todo/doing/review/done, `*` on the
/// default board — with the current board bold, then (under an `archived` line, dim) the
/// archived boards, which `r` restores and `d` deletes. An overlay like the `?` help: it never
/// moves the board underneath, and it scrolls (with the selection) when the pane is too short.
fn draw_boards(f: &mut Frame, app: &App, sel: usize) {
    use ratatui::widgets::{Cell, Row, Table};
    let live_n = app.boards.len();
    let arch_n = app.archived_boards.len();
    let names = app.boards.iter().map(|b| &b.name).chain(app.archived_boards.iter().map(|a| &a.name));
    // the `archived` line is laid out like a name, so it is never cut
    let head_w = if arch_n > 0 { BOARD_ARCHIVED_HEAD.len() } else { 0 };
    let longest = names.map(|n| n.chars().count()).chain([head_w]).max().unwrap_or(0) as u16;
    let lines = live_n + arch_n + usize::from(arch_n > 0);
    let area = centered(f.area(), boards_width(longest), lines as u16 + 3);
    if area.width < 10 || area.height < 4 {
        return;
    }
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(base_style(app)), area);
    // a hint that does not fit between the corners would be cut, so take the widest that does
    let room = area.width.saturating_sub(2) as usize;
    let hint = BOARD_HINTS.iter().find(|h| h.chars().count() <= room).copied().unwrap_or("");
    let b = frame(true, None)
        .title(Span::styled(" Boards ", bold()))
        .title_bottom(Line::styled(hint, bold()));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let pad = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    if live_n + arch_n == 0 {
        let lines: Vec<Line> = wrap_words("no boards yet", pad.width as usize).into_iter().map(Line::raw).collect();
        f.render_widget(Paragraph::new(lines), pad);
        return;
    }
    let (keep, name_w) = boards_plan(pad.width, longest);
    let sel = sel.min(live_n + arch_n - 1);
    let sel_st = bold().add_modifier(Modifier::REVERSED);
    let mut rows: Vec<Row> = app
        .boards
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let current = b.name == app.snap.board;
            let marker = format!("{}{}", if i == sel { ">" } else { " " }, if b.is_default { "*" } else { " " });
            let mut cells = vec![Cell::from(marker), Cell::from(fit(&b.name, name_w as usize))];
            for (k, (_, w)) in BOARD_COLS.iter().enumerate() {
                if keep[k] {
                    cells.push(Cell::from(format!("{:>w$}", b.counts[k], w = *w as usize)));
                }
            }
            let row = Row::new(cells);
            // the board you are on stays bold; the cursor row is reversed on top of that
            match (i == sel, current) {
                (true, _) => row.style(sel_st),
                (_, true) => row.style(bold()),
                _ => row,
            }
        })
        .collect();
    if arch_n > 0 {
        let head = if name_w as usize >= BOARD_ARCHIVED_HEAD.len() { BOARD_ARCHIVED_HEAD } else { "" };
        rows.push(Row::new([Cell::from(""), Cell::from(head)]).style(dim()));
        for (j, a) in app.archived_boards.iter().enumerate() {
            let i = live_n + j;
            let marker = if i == sel { ">" } else { " " };
            let mut cells = vec![Cell::from(marker), Cell::from(fit(&a.name, name_w as usize))];
            for (k, (_, w)) in BOARD_COLS.iter().enumerate() {
                if keep[k] {
                    let n = a.counts.map_or("-".to_string(), |c| c[k].to_string());
                    cells.push(Cell::from(format!("{n:>w$}", w = *w as usize)));
                }
            }
            let row = Row::new(cells);
            rows.push(if i == sel { row.style(sel_st) } else { row.style(dim()) });
        }
    }
    // a header is shown whole or not at all — never half of one
    let name_head = if name_w >= BOARD_NAME_HEAD.len() as u16 { BOARD_NAME_HEAD } else { "" };
    let mut head = vec![Cell::from(""), Cell::from(name_head)];
    let mut widths = vec![Constraint::Length(2), Constraint::Length(name_w)];
    for (k, (name, w)) in BOARD_COLS.iter().enumerate() {
        if keep[k] {
            head.push(Cell::from(*name));
            widths.push(Constraint::Length(*w));
        }
    }
    // scroll with the selection: the header stays, the rows below it slide
    let body_h = inner.height.saturating_sub(1) as usize;
    // the `archived` line sits between the two lists, so an archived row is one line lower
    let line = sel + usize::from(sel >= live_n);
    let off = (line + 1).saturating_sub(body_h.max(1));
    let rows: Vec<Row> = rows.into_iter().skip(off).collect();
    let table = Table::new(rows, widths).header(Row::new(head).style(bold())).column_spacing(1);
    f.render_widget(table, pad);
}

/// Row budget for the FULL/MEDIUM board: (github, agents, detail, github bar, agents bar).
/// Priority, highest first: boxed cards (columns >= 14 rows when the terminal is >= 30 rows,
/// else >= 10); then GITHUB (full, fewer rows / one-line tiles, then a 1-line bar); then
/// AGENTS (fewer rows, then a bar); the detail strip is dropped first. Bars are never dropped:
/// the columns give up rows for them.
/// `gh_full` is the GitHub panel's full height (0 = not shown).
pub fn board_budget(h: u16, gh_full: u16, want_ag: bool, want_detail: bool, agent_rows: u16) -> (u16, u16, u16, bool, bool) {
    let want_gh = gh_full > 0;
    let col_min = if h >= 30 { BOXED_MIN_ROWS as u16 + 2 } else { 10 };
    let rest = h.saturating_sub(2 + col_min); // header + footer + columns
    let ag_full = if want_ag { agent_rows + 2 } else { 0 };
    let detail = if want_detail { 4 } else { 0 };
    if gh_full + ag_full + detail <= rest {
        return (gh_full, ag_full, detail, false, false);
    }
    if gh_full + ag_full <= rest {
        return (gh_full, ag_full, 0, false, false); // detail strip goes first
    }
    // github shrinks (>= 5 rows: summary line + a few table rows), agents keep their size
    let gh_min = gh_full.min(5);
    if want_gh && rest >= ag_full + gh_min {
        return ((rest - ag_full).min(gh_full), ag_full, 0, false, false);
    }
    // github becomes a bar; agents shrink, then become a bar too
    let rest = rest.saturating_sub(u16::from(want_gh));
    let ag = if ag_full <= rest {
        ag_full
    } else if rest >= 3 {
        rest
    } else {
        0
    };
    (0, ag, 0, want_gh, want_ag && ag == 0)
}

fn draw_board(f: &mut Frame, app: &App, area: Rect, _adaptive: bool) {
    let wide = area.width >= NARROW;
    // one row per actor of this board plus the `+N elsewhere` line, as many as before (<= 8)
    let agent_rows = app.roster().panel_rows().clamp(1, 8) as u16;
    // the GITHUB panel shows even without a repo (a 1-line "pick one" panel), unless G hid it
    let want_gh = wide && app.show_github;
    let gh_full = match (want_gh, app.gh.repo.is_some()) {
        (false, _) => 0,
        (true, true) => GITHUB_ROWS,
        (true, false) => 3,
    };
    let want_ag = wide && app.show_agents;
    let mut budget = board_budget(area.height, gh_full, want_ag, wide, agent_rows);
    // Cards come first: when the columns would still hide cards, GITHUB gets only its
    // CONTENT height (never padding rows) and the rows it gives up go to the columns.
    let cols_h = |b: &(u16, u16, u16, bool, bool)| {
        let bars = u16::from(b.3 || (!wide && app.show_github)) + u16::from(b.4 || (!wide && app.show_agents));
        area.height.saturating_sub(2 + bars + b.0 + b.1 + b.2)
    };
    // (the compact GITHUB block then, sized to exactly what it draws)
    let content = layouts::gh_compact_height(app, area.width);
    let mut gh_compact = false;
    if want_gh && content < budget.0 && layouts::cells_hide(app, &layouts::four_cells(Rect { height: cols_h(&budget), ..area })) {
        budget = board_budget(area.height, content, want_ag, wide, agent_rows);
        gh_compact = true;
    }
    let (gh_h, ag_h, detail_h, gh_bar, ag_bar) = budget;
    // narrow MEDIUM (< 100 cols): panels still show, as bars
    let gh_bar = gh_bar || (!wide && app.show_github);
    let ag_bar = ag_bar || (!wide && app.show_agents);
    app.shown.set((gh_h > 0, ag_h > 0));
    app.bars.set((gh_bar, ag_bar));
    let mut cons = vec![Constraint::Length(1)];
    cons.push(Constraint::Min(1));
    if gh_bar {
        cons.push(Constraint::Length(1));
    }
    if ag_bar {
        cons.push(Constraint::Length(1));
    }
    if gh_h > 0 {
        cons.push(Constraint::Length(gh_h));
    }
    if ag_h > 0 {
        cons.push(Constraint::Length(ag_h));
    }
    if detail_h > 0 {
        cons.push(Constraint::Length(detail_h));
    }
    cons.push(Constraint::Length(1));
    let rows = Layout::vertical(cons).split(area);
    let mut i = 0;
    f.render_widget(Paragraph::new(header(app, area.width)), rows[i]);
    i += 1;
    layouts::draw_four(f, app, rows[i]);
    i += 1;
    if gh_bar {
        note_area(app, 0, rows[i]);
        f.render_widget(Paragraph::new(layouts::gh_bar(app, area.width as usize)), rows[i]);
        i += 1;
    }
    if ag_bar {
        note_area(app, 1, rows[i]);
        f.render_widget(Paragraph::new(layouts::ag_bar(app, area.width as usize)), rows[i]);
        i += 1;
    }
    if gh_h > 0 {
        if gh_compact {
            layouts::draw_gh_compact(f, app, rows[i]);
        } else {
            draw_github(f, app, rows[i]);
        }
        i += 1;
    }
    if ag_h > 0 {
        note_area(app, 1, rows[i]);
        let focused = app.focus == Focus::Agents;
        let b = frame(focused, None).title(Span::styled(" AGENTS ", bold()));
        let inner_w = rows[i].width.saturating_sub(2) as usize;
        let mut lines = close_clipped(agents_panel(app, inner_w), app, rows[i].height.saturating_sub(2) as usize, inner_w);
        if focused {
            let sel = app.ag_sel.min(lines.len().saturating_sub(1));
            if let Some(l) = lines.get_mut(sel) {
                *l = l.clone().patch_style(bold().add_modifier(Modifier::REVERSED));
            }
        }
        let inner_h = rows[i].height.saturating_sub(2) as usize;
        let off = (app.ag_sel + 1).saturating_sub(inner_h) as u16;
        f.render_widget(Paragraph::new(lines).block(b).scroll((if focused { off } else { 0 }, 0)), rows[i]);
        i += 1;
    }
    if detail_h > 0 {
        let b = frame(false, None);
        f.render_widget(Paragraph::new(detail_strip(app)).block(b), rows[i]);
        i += 1;
    }
    f.render_widget(Paragraph::new(footer(app, area.width)), rows[i]);
}





fn board_name(app: &App) -> &str {
    if app.snap.board.is_empty() {
        "default"
    } else {
        &app.snap.board
    }
}

// ---------- event loop ----------

pub fn run(mut store: Store, actor: &str) -> std::io::Result<()> {
    let snap = store.snapshot().map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut app = App::new(snap, actor);
    app.reload(&store);

    // GitHub: a background thread fetches every 60s while a repo is configured; the UI never waits.
    // The result carries the repo it is for: `B` can switch boards mid-fetch, and a snapshot
    // of the old board's repo must never be saved into the new board.
    type GhResult = (String, std::result::Result<github::GhSnapshot, String>, std::collections::HashMap<i64, github::RefState>);
    // repo + database file of the board the board that is running now (both change on `B`)
    let gh_target: Arc<Mutex<(Option<String>, Option<std::path::PathBuf>)>> =
        Arc::new(Mutex::new((app.gh.repo.clone(), store.path())));
    let gh_out: Arc<Mutex<Option<GhResult>>> = Arc::new(Mutex::new(None));
    {
        let (gh_repo, gh_out) = (Arc::clone(&gh_target), Arc::clone(&gh_out));
        let cached_age = app
            .gh
            .snap
            .as_ref()
            .map(|s| (crate::store::now() - s.fetched_at).max(0) as u64);
        std::thread::spawn(move || {
            let every = Duration::from_secs(github::MAX_AGE_SECS as u64);
            let mut last: Option<(String, Instant)> = None;
            let mut first = true;
            loop {
                let (repo, db_path) = gh_repo.lock().ok().map(|g| g.clone()).unwrap_or_default();
                if let Some(r) = repo {
                    if first {
                        // a fresh cache counts as the last fetch
                        if let Some(age) = cached_age.filter(|a| *a < every.as_secs()) {
                            last = Instant::now().checked_sub(Duration::from_secs(age)).map(|t| (r.clone(), t));
                        }
                        first = false;
                    }
                    let due = last.as_ref().is_none_or(|(lr, t)| *lr != r || t.elapsed() >= every);
                    if due {
                        let res = github::fetch(&r, crate::store::now());
                        // closed/merged evidence for board cards the open lists don't cover
                        let states = match (&res, &db_path) {
                            (Ok(snap), Some(p)) => Store::open(p)
                                .and_then(|st| st.list())
                                .map(|cards| github::fetch_states(&r, &github::needs_state(snap, &cards)))
                                .unwrap_or_default(),
                            _ => Default::default(),
                        };
                        if let Ok(mut g) = gh_out.lock() {
                            *g = Some((r.clone(), res, states));
                        }
                        last = Some((r, Instant::now()));
                    }
                }
                #[allow(clippy::disallowed_methods, reason = "the GitHub refresh thread's timer; not a write-path wait")]
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    let shared = Arc::new(Mutex::new(AgentsState::Pending));
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || loop {
            let s = herdr::probe();
            if let Ok(mut g) = shared.lock() {
                *g = s;
            }
            #[allow(clippy::disallowed_methods, reason = "the herdr refresh thread's timer; not a write-path wait")]
            std::thread::sleep(HERDR_EVERY);
        });
    }

    let mut terminal = ratatui::init();
    let mut last = Instant::now();
    let res = (|| -> std::io::Result<()> {
        loop {
            if let Ok(g) = shared.lock() {
                app.agents = g.clone();
            }
            terminal.draw(|f| draw(f, &app))?;
            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press && app.handle_key(k, &mut store) {
                        return Ok(());
                    }
                }
            }
            app.poll_repos();
            if let Ok(mut g) = gh_target.lock() {
                // a newly picked repo (or a board switched with B) is fetched right away
                g.0.clone_from(&app.gh.repo);
                g.1 = store.path();
            }
            let fetched = gh_out.lock().ok().and_then(|mut g| g.take());
            // a result for a repo this board no longer wants (B switched under it) is dropped
            if let Some((res, states)) = fetched
                .filter(|(repo, ..)| app.gh.repo.as_deref() == Some(repo.as_str()))
                .map(|(_, res, states)| (res, states))
            {
                let _ = store.save_github(&res);
                if let (Ok(snap), Ok(cards), Ok(returned)) = (&res, store.list(), store.returned_at()) {
                    let moves = github::plan_moves(snap, &cards, &states, &returned);
                    if !moves.is_empty() && github::apply_moves(&mut store, &moves).is_ok() {
                        app.status = Some((format!("github moved {} card(s): {}", moves.len(), moves[0].text), false));
                        app.status_until = Some(Instant::now() + Duration::from_secs(5));
                    }
                }
                app.reload(&store);
            }
            if app.status_until.is_some_and(|t| Instant::now() >= t) {
                app.status = None;
                app.status_until = None;
            }
            if last.elapsed() >= REFRESH {
                app.reload(&store);
                if let Ok(mut g) = gh_target.lock() {
                    g.0.clone_from(&app.gh.repo);
                    g.1 = store.path();
                }
                last = Instant::now();
            }
        }
    })();
    ratatui::restore();
    res
}

#[cfg(test)]
mod look_tests {
    use super::*;
    use crate::store::display::Display;

    fn snap(labels: [Option<&str>; 4], by_due: bool) -> Snapshot {
        Snapshot { display: Display { labels: labels.map(|l| l.map(str::to_string)), by_due, ..Default::default() }, ..Default::default() }
    }

    /// The header FUNCTION, snapshotted: a label at every room it can be given.
    #[test]
    fn a_label_gives_way_in_whole_words_and_the_plain_name_is_the_last_resort() {
        let s = snap([None, None, Some("WITH THE REVIEWER"), Some("FILED")], false);
        let at = |room: usize| column_name(&s, "review", room);
        assert_eq!(at(40), "WITH THE REVIEWER");
        assert_eq!(at(17), "WITH THE REVIEWER");
        assert_eq!(at(16), "WITH THE");
        assert_eq!(at(8), "WITH THE");
        assert_eq!(at(7), "WITH");
        assert_eq!(at(4), "WITH");
        assert_eq!(at(3), "REVIEW", "not even one word fits: the plain name, never half a word");
        for room in 0..40 {
            let name = at(room);
            assert!(name == "REVIEW" || (cells(&name) <= room && "WITH THE REVIEWER".starts_with(&name)), "room {room}: {name:?}");
            assert!(!name.ends_with(' '));
        }
        // DONE keeps its ` today` while it fits whole
        assert_eq!(column_name(&s, "done", 11), "FILED today");
        assert_eq!(column_name(&s, "done", 10), "FILED");
        // no label: the name tb always drew, whatever the room
        for room in [0, 3, 40] {
            assert_eq!(column_name(&s, "todo", room), "TODO");
            assert_eq!(column_name(&snap([None; 4], false), "done", room), "DONE today");
        }
    }

    #[test]
    fn wide_characters_count_as_the_cells_they_take() {
        assert_eq!(cells("審査中"), 6);
        assert_eq!(cells("OK ✅"), 5);
        let s = snap([None, None, Some("審査中 担当者"), None], false);
        assert_eq!(column_name(&s, "review", 13), "審査中 担当者");
        assert_eq!(column_name(&s, "review", 12), "審査中", "13 cells do not fit in 12");
        assert_eq!(column_name(&s, "review", 5), "REVIEW");
    }

    #[test]
    fn a_date_ordered_column_says_so_when_there_is_room() {
        let s = snap([None; 4], true);
        assert_eq!(date_order_note(&s, "todo", 7), "by due ");
        assert_eq!(date_order_note(&s, "review", 30), "by due ");
        assert_eq!(date_order_note(&s, "todo", 6), "", "it is the first thing a narrow header gives up");
        assert_eq!(date_order_note(&s, "doing", 30), "");
        assert_eq!(date_order_note(&snap([None; 4], false), "todo", 30), "");
    }
}

// A separate `mod` (not `look_tests`, above) so the doc comment on the module is about what
// it actually covers; both compile into the same `--lib` test binary either way, run
// multi-threaded by cargo's default harness. `crate::notice` is a process-wide static, but
// every push and drain here is KEYED to a board's own tempdir path (see `notice::push_for` /
// `take_unprinted_for`), and `tempfile::tempdir()` hands out a fresh, unique directory every
// call — so this test's keys can never collide with another test's, in this module or any
// other, running at the same time in the same process. An earlier version of this test drove
// `App::reload` through the OLD unkeyed `notice::take_unprinted()`, which is exactly the
// cross-test race that bit `tests/v1.rs::help_overlay_and_footer` in review: a notice pushed
// by an unrelated concurrently-running test leaked into this board's status line. Keying by
// path is what makes that structurally impossible now, not test-file discipline.
#[cfg(test)]
mod notice_tests {
    use super::*;

    /// Warnings raised for THIS board must reach the status line while it is open — not only
    /// stderr after it exits, which the alternate screen hides while it runs (card #83) — and
    /// never a warning raised for a different board, however it got pushed.
    #[test]
    fn a_pending_warning_reaches_the_status_line_and_survives_a_board_switch() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::store::Store::open(&dir.path().join("old.db")).unwrap();
        let key = store.notice_key().unwrap();
        let mut app = App::new(store.snapshot().unwrap(), "alice");
        assert!(app.status.is_none());

        // a notice already pending under this board's own key when it opens reaches the
        // status line on the next reload (the same reload the run loop already calls every
        // couple of seconds); one pushed under an unrelated key must never appear here —
        // that is the cross-board leak a keyed drain exists to rule out
        crate::notice::push_for(&key, "card-83-test-1: the board file is open to other users");
        crate::notice::push_for("card-83-test-other-board", "must never reach a different board's status line");
        app.reload(&store);
        let (msg, is_err) = app.status.clone().expect("a pending warning must reach the status line, not only stderr after exit");
        assert!(is_err, "a warning carries the weight of an error, not a quiet aside: {msg}");
        assert!(msg.contains("card-83-test-1"), "{msg}");
        assert!(!msg.contains("must never reach"), "a keyed drain must never take another board's entry: {msg}");
        // it is shown once: a second reload with nothing new pending leaves it exactly as it
        // was — an unread warning is never silently replaced
        app.reload(&store);
        assert_eq!(app.status.as_ref().unwrap().0, msg);

        // switching board must not let its own "board: NAME" confirmation silently swallow a
        // warning the new open just raised, before the next frame ever draws it
        app.status = None;
        let new_path = dir.path().join("new.db");
        let new_key = crate::store::Store::open(&new_path).unwrap().notice_key().unwrap();
        crate::notice::push_for(&new_key, "card-83-test-2: a warning raised opening the new board");
        let row = crate::boards::BoardRow { name: "new".into(), is_default: false, counts: [0; 4], path: new_path };
        app.switch_board(&row, &mut store);
        let (msg2, is_err2) = app.status.clone().expect("the warning must win over the plain confirmation");
        assert!(is_err2, "{msg2}");
        assert!(msg2.contains("card-83-test-2"), "{msg2}");

        // the other board's entry pushed above is still there, unread — a board that never
        // gets rendered in this process must not lose its own warning either
        let other = crate::notice::take_unprinted_for("card-83-test-other-board");
        assert!(other.iter().any(|m| m.contains("must never reach")), "{other:?}");
    }
}

#[cfg(test)]
mod even_split_tests {
    use super::*;

    /// THE LAYOUT INVARIANT, as a property: for every total and pane count, the extents add
    /// up to the total and differ by at most one — and they land on the same cells ratatui's
    /// `Constraint::Ratio(1, n)` would, so switching a split to it moves nothing.
    #[test]
    fn extents_are_even_add_up_and_match_ratatui_ratio() {
        for n in 1..=4usize {
            for total in 0..=300u16 {
                let e = even_extents(total, n);
                assert_eq!(e.len(), n);
                assert_eq!(e.iter().sum::<u16>(), total, "{total}/{n}: {e:?}");
                assert!(e.iter().max().unwrap() - e.iter().min().unwrap() <= 1, "{total}/{n}: {e:?}");
                let area = Rect { x: 0, y: 0, width: total, height: 1 };
                let ratio: Vec<u16> = Layout::horizontal(vec![Constraint::Ratio(1, n as u32); n]).split(area).iter().map(|r| r.width).collect();
                assert_eq!(e, ratio, "{total}/{n}");
                let rects = split_even(area, n, true);
                assert_eq!(rects.iter().map(|r| r.width).collect::<Vec<_>>(), e);
                assert!(rects.windows(2).all(|p| p[0].x + p[0].width == p[1].x));
            }
        }
    }
}
