//! Interactive board (ratatui). Pure `draw` over `App` so it can be tested with TestBackend.

use crate::github::{self, GhView};
use crate::herdr::{self, Agent, AgentsState};
use crate::plain::{card_head, event_line, fit, meta_fit};
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
    ("third-v", "tall (cols < rows*2.2), cols <= 62, rows >= 30"),
    ("half-h", "wide, rows >= 30"),
    ("half-v", "tall, cols >= 63, rows >= 30"),
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
        } else if w <= 62 {
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
    Note { id: i64, buf: String, from_popup: bool },
    /// Sending a REVIEW card back to DOING: the reason being typed.
    SendBack { id: i64, buf: String },
    Popup(i64),
    AddCheck { id: i64, buf: String },
    /// Repo picker (`R`): typed filter and selected row (row 0 = "none").
    Picker { filter: String, sel: usize },
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
    /// Approve a REVIEW card the actor moved to review themselves (the forced, logged path).
    ApproveOwn(i64),
}

/// Title + description edit form (`e`). `cursor` is a char index into the active field.
#[derive(Debug, Clone, PartialEq)]
pub struct EditForm {
    pub id: i64,
    pub title: String,
    pub desc: String,
    /// 0 = title, 1 = description
    pub field: u8,
    pub cursor: usize,
    pub from_popup: bool,
}

impl EditForm {
    fn text(&mut self) -> &mut String {
        if self.field == 0 {
            &mut self.title
        } else {
            &mut self.desc
        }
    }

    /// Apply one editing key; returns false if the key isn't an edit key.
    pub fn key(&mut self, code: KeyCode) -> bool {
        let len = if self.field == 0 { self.title.chars().count() } else { self.desc.chars().count() };
        let cur = self.cursor.min(len);
        match code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.field = 1 - self.field;
                self.cursor = if self.field == 0 { self.title.chars().count() } else { self.desc.chars().count() };
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
            popup: None,
            status: None,
            actor: actor.to_string(),
            cursor: 0,
            last_shape: std::cell::Cell::new(Shape::HalfH),
            status_until: None,
            repos: RepoState::Idle,
            repos_rx: None,
            picker_msg: None,
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
                    self.mode = Mode::Normal;
                    if !buf.trim().is_empty() {
                        let r = store.add(&buf, "", &[], &actor);
                        if let Some(id) = self.report(r, |id| format!("added #{id}")) {
                            self.reload(store);
                            self.focus_card(id);
                        }
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
                        let r = store.add_check(id, &buf, &actor);
                        if let Some(n) = self.report(r, |n| format!("#{id} item {n} added")) {
                            self.reload(store);
                            self.cursor = (n as usize).saturating_sub(1);
                        }
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
                            let r = store.check(id, n, &actor);
                            self.report(r, |d| {
                                format!("#{id} item {n} {}", if *d { "checked" } else { "unchecked" })
                            });
                            self.reload(store);
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
                            let r = store.remove_check(id, n, &actor);
                            if self.report(r, |_| format!("#{id} item {n} deleted")).is_some() {
                                self.reload(store);
                                self.cursor = self.cursor.min(items.len().saturating_sub(2));
                            }
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
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')) {
                    self.mode = Mode::Normal;
                }
            }
            Mode::Confirm { action, .. } => {
                self.mode = Mode::Normal;
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    match action {
                        Confirm::Delete(id) => {
                            let r = store.delete_card(id, &actor);
                            if self.report(r, |c| format!("deleted #{} \"{}\"", c.id, c.title)).is_some() {
                                self.reload(store);
                            }
                        }
                        Confirm::ForceDone(id) => {
                            if self.is_own_review(id, store) {
                                self.mode = approve_own(id);
                                return false;
                            }
                            let r = store.move_to(id, "done", &actor);
                            if self.report(r, |c| format!("#{} -> done", c.id)).is_some() {
                                self.reload(store);
                                self.focus_card(id);
                            }
                        }
                        Confirm::ApproveOwn(id) => {
                            let r = store.move_to_forced(id, "done", &actor);
                            if self.report(r, |c| format!("#{} -> done (own work, logged)", c.id)).is_some() {
                                self.reload(store);
                                self.focus_card(id);
                            }
                        }
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
                    let r = store.edit(form.id, Some(&form.title), Some(&form.desc), &actor);
                    if self.report(r, |c| format!("#{} saved", c.id)).is_some() {
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

    fn open_edit(&mut self, id: i64, from_popup: bool) {
        if let Some(c) = self.snap.cards.iter().find(|c| c.id == id) {
            let title = crate::store::raw_title(c);
            let cursor = title.chars().count();
            self.mode = Mode::Edit(EditForm { id, title, desc: c.description.clone(), field: 0, cursor, from_popup });
        }
    }

    /// Would moving `card` to done skip GitHub's evidence (issue/PR still open)?
    fn open_on_github(&self, id: i64) -> Option<i64> {
        let c = self.snap.cards.iter().find(|c| c.id == id)?;
        let n = c.gh_ref?;
        let snap = self.gh.snap.as_ref()?;
        github::still_open(snap, n).then_some(n)
    }

    /// Is `id` a REVIEW card this actor authored (moved to review themselves)?
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
        if to == "done" && column != "done" {
            if let Some(n) = self.open_on_github(id) {
                self.mode = Mode::Confirm {
                    action: Confirm::ForceDone(id),
                    prompt: format!("issue #{n} still open on GitHub — mark done anyway? y/n"),
                };
                return;
            }
            if self.is_own_review(id, store) {
                self.mode = approve_own(id);
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
        let actor = self.actor.clone();
        let r = store.reorder(id, how, &actor);
        if self.report(r, |_| format!("#{id} moved {how}")).is_some() {
            self.reload(store);
            self.focus_card(id);
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

    fn agent_card(&self, i: usize) -> Option<&Card> {
        let agents = match &self.agents {
            AgentsState::Agents(a) => a,
            _ => return None,
        };
        let a = agents.get(i)?;
        let mine = |c: &&Card| herdr::find_owner(agents, c).is_some_and(|o| o.pane_id == a.pane_id);
        let owned: Vec<&Card> = self.snap.cards.iter().filter(|c| c.column != "done").filter(mine).collect();
        owned.iter().find(|c| c.column == "doing").or(owned.first()).copied()
    }

    fn open_picker(&mut self, preselect: bool) {
        self.picker_msg = None;
        self.start_repo_load();
        self.picker_preselect = preselect;
        self.mode = Mode::Picker { filter: String::new(), sel: 0 };
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
        let n_agents = match &self.agents {
            AgentsState::Agents(a) => a.len(),
            _ => 0,
        };
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
            (_, KeyCode::Char(c @ ('T' | 'L' | 'A' | 'G' | 'R'))) => {
                let prev = self.focus;
                self.focus = Focus::Columns;
                let q = self.normal_key(KeyEvent::new(KeyCode::Char(c), key.modifiers), store);
                if !matches!(self.mode, Mode::Picker { .. }) {
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
                    let actor = self.actor.clone();
                    let r = store.check(id, n, &actor);
                    self.report(r, |d| format!("#{id} item {n} {}", if *d { "checked" } else { "unchecked" }));
                    self.reload(store);
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
                    self.mode = Mode::Confirm {
                        action: Confirm::Delete(c.id),
                        prompt: format!("delete #{} \"{}\"? y/n", c.id, c.title),
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
    let agents = agent_list(app);
    let working = agents.iter().filter(|a| a.status == "working").count();
    let idle = agents.iter().filter(|a| a.is_idle()).count();
    let left = format!(
        " TERMINAL BOARD · {} · {} cards · {} agents ({working} working, {idle} idle)",
        if app.snap.board.is_empty() { "default" } else { &app.snap.board },
        app.snap.cards.len(),
        agents.len()
    );
    let mut right = format!("refreshed {} ", clock_secs(app.snap.now));
    if left.chars().count() + right.chars().count() > width as usize {
        right.clear(); // no room: drop the clock rather than cut it
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

    let (base, warn) = meta_fit(card, &app.snap, width.saturating_sub(indent.len() + meta_gh.chars().count()));
    let owner_style = match owner_agent(app, card) {
        Some(a) if a.status == "working" => Style::default(),
        Some(a) if a.status == "blocked" => bold(),
        _ => dim(),
    };
    let sep = if base.is_empty() || warn.is_empty() { "" } else { " " };
    let mut second = vec![Span::raw(indent)];
    if !meta_gh.is_empty() {
        second.push(Span::raw(meta_gh));
    }
    second.push(Span::styled(base, owner_style));
    second.push(Span::raw(sep));
    if !warn.is_empty() {
        second.push(Span::styled(warn, red()));
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

fn owner_agent<'a>(app: &'a App, card: &Card) -> Option<&'a Agent> {
    herdr::find_owner(agent_list(app), card)
}

/// Does column `ci` in `area` need the dense (title-in-border) card style to fit?
fn column_needs_dense(app: &App, ci: usize, area: Rect) -> bool {
    let cards = app.col_cards(ci);
    let inner_h = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2);
    if cards.is_empty() || inner_h < 3 || inner_w < 8 {
        return false;
    }
    let text_w = inner_w.saturating_sub(4) as usize;
    let full: usize = cards.iter().map(|c| card_lines(app, c, false, text_w, true).len() + 2).sum();
    full > inner_h
}

/// One card style per render: dense everywhere if any column needs it.
pub(crate) fn any_dense(app: &App, cols: &[(usize, Rect)]) -> bool {
    cols.iter().any(|(ci, r)| column_needs_dense(app, *ci, *r))
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

fn draw_column(f: &mut Frame, app: &App, ci: usize, area: Rect, dense: bool) {
    note_col(app, ci, area);
    let col = COLUMNS[ci];
    let cards = app.col_cards(ci);
    let focused = ci == app.col;
    let n = cards.len();
    let count = if col == "doing" { format!("{n}/{}", app.snap.wip) } else { n.to_string() };
    let name = if col == "done" { "DONE today".to_string() } else { col.to_ascii_uppercase() };
    let full = col == "doing" && n as i64 >= app.snap.wip;
    let colour = column_colour_in(col, &app.snap.theme);
    let hs = bold().fg(colour);
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled("o", Style::default().fg(colour)),
        Span::styled(format!(" {name} ("), hs),
        Span::styled(count, if full { hs.add_modifier(Modifier::REVERSED) } else { hs }),
        Span::styled(") ", hs),
    ]);
    // Column frame: plain fg (cards carry the colour now; less busy), thick when focused.
    // Column frame in the column's colour (same as its cards), thick when focused.
    let block = frame(focused, Some(colour)).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let sel = if focused { Some(app.row[ci].min(n.saturating_sub(1))) } else { None };
    if cards.is_empty() {
        f.render_widget(Paragraph::new(Line::styled(" -", dim())), inner);
    } else if inner.height < 3 || inner.width < 8 {
        app.drawn_styles.borrow_mut().push((ci, "compact"));
        draw_compact(f, app, &cards, sel, inner);
    } else {
        let used_dense = draw_boxed(f, app, &cards, sel, inner, colour, dense);
        app.drawn_styles.borrow_mut().push((ci, if used_dense { "dense" } else { "full" }));
    }
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

fn draw_compact(f: &mut Frame, app: &App, cards: &[&Card], sel: Option<usize>, inner: Rect) {
    let mut lines = Vec::new();
    let mut sel_end = 0;
    for (i, c) in cards.iter().enumerate() {
        lines.extend(card_lines(app, c, sel == Some(i), inner.width as usize, false));
        if sel == Some(i) {
            sel_end = lines.len();
        }
    }
    let offset = sel_end.saturating_sub(inner.height as usize) as u16;
    f.render_widget(Paragraph::new(lines).scroll((offset, 0)), inner);
}

/// Trello-style: each card in its own box in the column colour; the selected one thick.
/// The confirm line for approving your own REVIEW card (a solo person is not trapped).
fn approve_own(id: i64) -> Mode {
    Mode::Confirm {
        action: Confirm::ApproveOwn(id),
        prompt: "you moved this to review yourself — approve your own work? y/n".into(),
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
    // tight: carry each card's first line in its top border (3 rows per card instead of 4)
    let dense = dense || bodies.iter().map(|b| b.len() + 2).sum::<usize>() > avail;
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
        let hint = Line::styled(format!(" +{start} more"), dim());
        f.render_widget(Paragraph::new(hint), Rect { y, height: 1, ..inner });
        y += 1;
    }
    let mut i = start;
    while i < cards.len() {
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
            y += 1;
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
    if i < cards.len() && y < bottom {
        let hint = Line::styled(format!(" +{} more", cards.len() - i), dim());
        f.render_widget(Paragraph::new(hint), Rect { y: bottom - 1, height: 1, ..inner });
    }
    dense
}

fn agents_panel(app: &App) -> Vec<Line<'static>> {
    let agents = match &app.agents {
        AgentsState::Agents(a) => a,
        AgentsState::Pending => return vec![Line::styled(" checking herdr...", dim())],
        AgentsState::Unavailable(msg) => return vec![Line::styled(format!(" {msg}"), dim())],
    };
    if agents.is_empty() {
        return vec![Line::styled(" no agent panes in herdr", dim())];
    }
    agents
        .iter()
        .map(|a| {
            let owned: Vec<&Card> = app
                .snap
                .cards
                .iter()
                .filter(|c| {
                    c.column != "done"
                        && herdr::find_owner(agents, c).is_some_and(|o| o.pane_id == a.pane_id)
                })
                .collect();
            let doing = owned.iter().find(|c| c.column == "doing");
            let holds = a.is_idle() && doing.is_some();
            let (mark, st) = if holds {
                ("!", red())
            } else {
                match a.status.as_str() {
                    "working" => ("*", Style::default().fg(GREEN)),
                    // a blocked agent is shown by its mark only; red is reserved for cards
                    "blocked" => ("x", bold()),
                    "idle" | "done" => ("-", dim()),
                    _ => ("?", dim()),
                }
            };
            // colour only the warning marks; the rest of the row stays monochrome
            let text_st = match (holds, a.status.as_str()) {
                (true, _) => bold(),
                (_, "working") => Style::default(),
                _ => st,
            };
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(mark, st),
                Span::raw(" "),
                Span::styled(format!("{:<14} ", fit(&a.name, 14)), text_st.add_modifier(Modifier::BOLD)),
                Span::raw(format!("{:<7.7} ", a.harness)),
                Span::styled(format!("{:<8.8} ", a.status), text_st),
            ];
            match doing.or(owned.first()) {
                Some(c) => {
                    spans.push(Span::raw(format!("#{:<4} ", c.id)));
                    if let Some(n) = crate::store::shown_ref(c) {
                        spans.push(Span::raw(format!("gh#{n} ")));
                    }
                    spans.push(Span::raw(format!("{:<28} ", fit(&c.title, 28))));
                    spans.push(Span::raw(format!("{:>5} ", crate::store::coarse_age(app.snap.now - c.column_since))));
                    if holds {
                        spans.push(Span::styled("! idle, holds card", st));
                    } else if let Some(n) = app.snap.last_note.get(&c.id) {
                        spans.push(Span::styled(format!("\"{n}\""), dim()));
                    }
                }
                None => spans.push(Span::styled(
                    a.job.clone().unwrap_or_else(|| "-".into()),
                    dim(),
                )),
            }
            Line::from(spans)
        })
        .collect()
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
            Span::styled("   enter save  esc cancel  (tip: 'admin: renew domain')", dim()),
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
            // the essentials; `?` has the rest
            let mut hints = vec![("a", "add"), ("e", "edit"), ("x", "del"), ("enter", "open"), ("shift+arrows", "move")];
            if app.col == 1 {
                hints.push(("+/-", "limit"));
            }
            if app.gh.repo.is_none() && app.show_github {
                hints.push(("R", "github: pick repo"));
            }
            hints.extend([("?", "help"), ("q", "quit")]);
            if hints_len(&hints) > width as usize {
                hints = vec![("a", "add"), ("enter", "open"), ("?", "help"), ("q", "quit")];
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
        let tiles = github::tiles(s, &fac, now);
        let row = Rect { y, height: 4, ..inner };
        let cells = Layout::horizontal([Constraint::Ratio(1, 4); 4]).spacing(1).split(row);
        for (k, (title, value, line2)) in tiles.into_iter().enumerate() {
            let v_style = if value == "FAIL" { red() } else { bold() };
            // narrow tiles: "PULL REQUESTS" -> "PRS" so the value stays whole
            let iw = cells[k].width.saturating_sub(4) as usize;
            let title = if title.len() + 2 + value.len() > iw && title == "PULL REQUESTS" { "PRS".to_string() } else { title };
            let lines = vec![
                Line::from(vec![Span::styled(format!("{title}  "), bold()), Span::styled(value, v_style)]),
                Line::raw(line2),
            ];
            let b = frame(false, None).padding(Padding::horizontal(1));
            f.render_widget(Paragraph::new(lines).block(b), cells[k]);
        }
        y += 4;
    } else if y < bottom {
        let spans: Vec<Span> = std::iter::once(Span::raw(" "))
            .chain(github::compact_summary(s, &fac).into_iter().map(|(t, r)| if r { Span::styled(t, red()) } else { Span::raw(t) }))
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
    let age = |ts: &str| github::age_of(ts, now).map(crate::store::coarse_age).unwrap_or_else(|| "?".into());
    // row budget: PRs get up to a third (min 1 if any), issues the rest (+1 for "+N more")
    let pr_rows = if s.prs.is_empty() { 0 } else { s.prs.len().min((avail.saturating_sub(2) / 3).max(1)) };
    let pr_h = if pr_rows > 0 { pr_rows + 1 } else { 0 };
    let issue_space = avail.saturating_sub(pr_h);
    // PR table: PR 6 · TITLE flex · CI 5 · REVIEW 7 · AGE 5 · BRANCH / ISSUE 30
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
                    Some((i, who)) => format!("{} -> #{i} ({who})", p.head_ref),
                    None => p.head_ref.clone(),
                };
                let draft = if p.is_draft { "(draft) " } else { "" };
                let cells = vec![
                    Cell::from(format!("#{}", p.number)),
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
        let header = Row::new(keep_cells(["PR", "TITLE", "CI", "REVIEW", "AGE", "BRANCH / ISSUE"].to_vec(), &pr_keep)).style(bold());
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
                Cell::from(format!("#{}", r.number)),
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
    let header = Row::new(keep_cells(["ISSUE", "TITLE", "STATE", "WHO", "AGE", "LABELS"].to_vec(), &is_keep)).style(bold());
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
const PR_COLS: [(&str, u16); 6] = [("pr", 6), ("title", 0), ("ci", 5), ("review", 7), ("age", 5), ("branch", 30)];
const ISSUE_COLS: [(&str, u16); 6] = [("issue", 6), ("title", 0), ("state", 15), ("who", 8), ("age", 5), ("labels", 16)];
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


fn holds_card(app: &App, a: &Agent) -> bool {
    a.is_idle()
        && app.snap.cards.iter().any(|c| {
            c.column == "doing" && herdr::find_owner(agent_list(app), c).is_some_and(|o| o.pane_id == a.pane_id)
        })
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
fn draw_edit(f: &mut Frame, app: &App, form: &EditForm) {
    let area = centered(f.area(), 80, 14);
    if area.width < 10 || area.height < 8 {
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
    let title_active = form.field == 0;
    f.render_widget(Paragraph::new(label(title_active, "Title  (tag: prefix sets the tag)")), Rect { height: 1, ..pad });
    let tb = frame(title_active, None).padding(Padding::horizontal(1));
    let title_rect = Rect { y: pad.y + 1, height: 3, ..pad };
    // keep the cursor visible in a long title
    let w = title_rect.width.saturating_sub(4) as usize;
    let skip = if title_active { form.cursor.saturating_sub(w.saturating_sub(1)) } else { 0 };
    let shown: String = form.title.chars().skip(skip).collect();
    f.render_widget(Paragraph::new(cursor_line(&shown, form.cursor - skip.min(form.cursor), title_active)).block(tb), title_rect);
    f.render_widget(Paragraph::new(label(!title_active, "Description")), Rect { y: pad.y + 4, height: 1, ..pad });
    let db = frame(!title_active, None).padding(Padding::horizontal(1));
    let desc_rect = Rect { y: pad.y + 5, height: pad.height.saturating_sub(5), ..pad };
    f.render_widget(
        Paragraph::new(cursor_line(&form.desc, form.cursor, !title_active)).block(db).wrap(Wrap { trim: false }),
        desc_rect,
    );
}

/// Every key, grouped, plus the CLI verbs.
pub const HELP_GROUPS: [(&str, &[(&str, &str)]); 6] = [
    ("Board", &[
        ("arrows", "select a card (left/right column, up/down card)"),
        ("shift+left/right", "move the card to the next column (also > <)"),
        ("shift+up/down, K J", "reorder the card within its column"),
        ("tab / shift+tab", "cycle focus: columns, GITHUB, AGENTS"),
        ("q", "quit"),
    ]),
    ("Cards", &[
        ("a", "add a card ('tag: title')"),
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

fn draw_help(f: &mut Frame, app: &App) {
    let rows: usize = HELP_GROUPS.iter().map(|g| g.1.len() + 1).sum();
    let area = centered(f.area(), 96, rows as u16 + 3);
    if area.width < 10 || area.height < 4 {
        return;
    }
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(base_style(app)), area);
    let b = frame(true, None)
        .title(Span::styled(" Terminal Board keys ", bold()))
        .title_bottom(Line::styled(" esc or ? closes ", bold()));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let kw = 26;
    let mut lines = Vec::new();
    for (group, keys) in HELP_GROUPS {
        lines.push(Line::styled(format!(" {group}"), bold()));
        for (k, d) in keys {
            lines.push(Line::from(vec![Span::styled(format!("   {k:<kw$}"), bold()), Span::raw(d.to_string())]));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
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

/// Enter on an agent row in the AGENTS panel.
fn draw_agent_info(f: &mut Frame, app: &App, i: usize) {
    let AgentsState::Agents(agents) = &app.agents else { return };
    let Some(a) = agents.get(i) else { return };
    let mut lines = vec![
        Line::raw(format!("harness {} · status {} · pane {}", a.harness, a.status, a.pane_id)),
        Line::raw(format!("job {}", a.job.clone().unwrap_or_else(|| "-".into()))),
    ];
    let hint = match app.agent_card(i) {
        Some(c) => {
            lines.push(Line::raw(format!("holds #{} {} ({})", c.id, c.title, c.column)));
            " enter jump to card  esc close "
        }
        None => {
            lines.push(Line::styled("holds no card", dim()));
            " esc close "
        }
    };
    info_popup(f, app, format!(" {} ", a.name), lines, hint);
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
    let agent_rows = match &app.agents {
        AgentsState::Agents(a) if !a.is_empty() => a.len().min(8) as u16,
        _ => 1,
    };
    // the GITHUB panel shows even without a repo (a 1-line "pick one" panel), unless G hid it
    let want_gh = wide && app.show_github;
    let gh_full = match (want_gh, app.gh.repo.is_some()) {
        (false, _) => 0,
        (true, true) => GITHUB_ROWS,
        (true, false) => 3,
    };
    let want_ag = wide && app.show_agents;
    let (gh_h, ag_h, detail_h, gh_bar, ag_bar) = board_budget(area.height, gh_full, want_ag, wide, agent_rows);
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
    let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(rows[i]);
    let dense = any_dense(app, &(0..4).map(|ci| (ci, cols[ci])).collect::<Vec<_>>());
    for ci in 0..4 {
        draw_column(f, app, ci, cols[ci], dense);
    }
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
        draw_github(f, app, rows[i]);
        i += 1;
    }
    if ag_h > 0 {
        note_area(app, 1, rows[i]);
        let focused = app.focus == Focus::Agents;
        let b = frame(focused, None).title(Span::styled(" AGENTS ", bold()));
        let mut lines = agents_panel(app);
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
    type GhResult = (std::result::Result<github::GhSnapshot, String>, std::collections::HashMap<i64, github::RefState>);
    let db_path = store.path();
    let gh_repo: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(app.gh.repo.clone()));
    let gh_out: Arc<Mutex<Option<GhResult>>> = Arc::new(Mutex::new(None));
    {
        let (gh_repo, gh_out) = (Arc::clone(&gh_repo), Arc::clone(&gh_out));
        let db_path = db_path.clone();
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
                let repo = gh_repo.lock().ok().and_then(|g| g.clone());
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
                            *g = Some((res, states));
                        }
                        last = Some((r, Instant::now()));
                    }
                }
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
            if let Ok(mut g) = gh_repo.lock() {
                // a newly picked repo is fetched right away (the thread sees the change)
                g.clone_from(&app.gh.repo);
            }
            let fetched = gh_out.lock().ok().and_then(|mut g| g.take());
            if let Some((res, states)) = fetched {
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
                if let Ok(mut g) = gh_repo.lock() {
                    g.clone_from(&app.gh.repo);
                }
                last = Instant::now();
            }
        }
    })();
    ratatui::restore();
    res
}
