//! Compact layouts for a third of the screen: the RAIL (wide and short: columns left, GITHUB
//! and AGENTS stacked on the right), the STACK (tall and narrow: the columns as stacked boxed
//! sections, then GITHUB, then AGENTS), 1-line BARS when a panel truly doesn't fit, and the
//! full-screen Tab VIEWS. Same boxed, coloured look as the full layout, just denser.

use super::*;

/// Tab target: the board, or one panel full screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Board,
    Github,
    Agents,
}

const TAB_HINT: &str = "   tab >";

/// `GITHUB acme/widgets · 10 issues (6 free) · 1 PR (1 FAIL) · merged 7   tab >`
pub(super) fn gh_bar(app: &App, width: usize) -> Line<'static> {
    let focused = app.focus == Focus::Github && app.view == View::Board;
    let base = if focused { bold().add_modifier(Modifier::REVERSED) } else { Style::default() };
    let mut spans = vec![Span::styled(" GITHUB ", bold().patch(base))];
    match (&app.gh.repo, &app.gh.snap) {
        (None, _) => spans.push(Span::styled("no repo — enter to pick one", base)),
        (Some(r), None) => spans.push(Span::styled(format!("{r} · {}", app.gh.error.clone().unwrap_or_else(|| "fetching...".into())), base)),
        (Some(r), Some(s)) => {
            let fac = github::factory(s, &app.snap.cards, app.snap.now);
            spans.push(Span::styled(format!("{r} · {} issues ({} free) · {} PR", s.issues_open, fac.unclaimed, s.prs.len()), base));
            if fac.failing > 0 {
                spans.push(Span::styled(format!(" ({} ", fac.failing), base));
                spans.push(Span::styled("FAIL", red().patch(base)));
                spans.push(Span::styled(")", base));
            }
            spans.push(Span::styled(format!(" · merged {}", s.merged_today.len()), base));
        }
    }
    spans.push(Span::styled(TAB_HINT, dim().patch(base)));
    fit_line(spans, width)
}

/// `AGENTS 3 here · 2 elsewhere (! bot-2 idle w/ card)   tab >`
pub(super) fn ag_bar(app: &App, width: usize) -> Line<'static> {
    let focused = app.focus == Focus::Agents && app.view == View::Board;
    let base = if focused { bold().add_modifier(Modifier::REVERSED) } else { Style::default() };
    let mut spans = vec![Span::styled(" AGENTS ", bold().patch(base))];
    let r = app.roster();
    match &app.agents {
        // nobody on this board and no agent pane anywhere: the bar reads as it always has
        AgentsState::Agents(_) if r.total() == 0 => spans.push(Span::styled("0 working · 0 idle", base)),
        AgentsState::Pending if r.total() == 0 => spans.push(Span::styled("checking herdr...", base)),
        AgentsState::Unavailable(m) if r.total() == 0 => spans.push(Span::styled(m.clone(), base)),
        _ => {
            let (here, elsewhere) = crate::roster::counts(&r);
            let held: Vec<&crate::roster::Row> = r.here.iter().filter(|row| row.idle_holder()).collect();
            let holders: Vec<String> = held.iter().map(|row| row.name.clone()).collect();
            let warning = format!("! {} idle w/ card", holders.join(", "));
            // a narrow bar gives up whole parts in this order: the idle durations, then
            // `· N elsewhere`, then (as ever) the `tab >` hint — who is here, and who is stuck,
            // matter more than how many agents are somewhere else
            let rest = if held.is_empty() { 0 } else { 3 + warning.chars().count() } + TAB_HINT.chars().count();
            let both = format!("{here} · {elsewhere}");
            let roomy = 8 + both.chars().count() + rest <= width;
            spans.push(Span::styled(if roomy { both } else { here }, base));
            if !held.is_empty() {
                // the durations follow the warning, so a narrow bar drops them before the words
                let ages: Vec<String> = held
                    .iter()
                    .map(|row| crate::tui::idle_hold_age(app, row))
                    .filter(|s| !s.is_empty())
                    .map(|s| s.trim_matches(|c| c == ' ' || c == '(' || c == ')').to_string())
                    .collect();
                let ages = if ages.is_empty() { String::new() } else { format!(" ({})", ages.join(", ")) };
                spans.push(Span::styled(" (", base));
                spans.push(Span::styled(warning, red().patch(base)));
                // shown whole or not at all, and never at the cost of the `tab >` hint: a bar
                // without room for all of it keeps the plain warning
                let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                let rest = 1 + TAB_HINT.chars().count();
                if roomy && !ages.is_empty() && used + ages.chars().count() + rest <= width {
                    spans.push(Span::styled(ages, red().patch(base)));
                }
                spans.push(Span::styled(")", base));
            }
        }
    }
    spans.push(Span::styled(TAB_HINT, dim().patch(base)));
    fit_line(spans, width)
}

/// Cut a line of spans to `width` chars (the last visible span gets an ellipsis).
pub(super) fn fit_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut left = width;
    for s in spans {
        if left == 0 {
            break;
        }
        let n = s.content.chars().count();
        if n <= left {
            left -= n;
            out.push(s);
        } else {
            out.push(Span::styled(fit(&s.content, left), s.style));
            left = 0;
        }
    }
    Line::from(out)
}

/// Four tiles, 2x2, each 3 rows with the label in its border (half-v, narrow full panels).
pub(super) fn draw_dense_tiles(f: &mut Frame, app: &App, s: &github::GhSnapshot, area: Rect) {
    let fac = github::factory(s, &app.snap.cards, app.snap.now);
    // a full page's label only where the whole tile line fits (long, else terse); else the
    // unlabelled line, cut as ever — the label never costs a count its place
    let tiles = github::tiles_as(s, &fac, app.snap.now, github::PageLabel::None);
    let labelled = github::PageLabel::LABELLED.map(|l| github::tiles_as(s, &fac, app.snap.now, l));
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).split(area);
    for (k, (title, value, line2)) in tiles.into_iter().enumerate() {
        let halves = Layout::horizontal([Constraint::Ratio(1, 2); 2]).spacing(1).split(rows[k / 2]);
        let cell = halves[k % 2];
        let room = cell.width.saturating_sub(4) as usize;
        let fitting = labelled
            .iter()
            .map(|f| (f[k].1.clone(), f[k].2.clone()))
            .find(|(v, l)| (*v != value || *l != line2) && v.chars().count() + 3 + l.chars().count() <= room);
        let (value, line2) = fitting.unwrap_or((value, line2));
        let v_style = if value == "FAIL" { red() } else { bold() };
        let fg = palette(&app.snap.theme).fg;
        let label = if title == "PULL REQUESTS" { "PRS".to_string() } else { title };
        let b = frame(false, None).title(Span::styled(format!(" {label} "), bold().fg(fg))).padding(Padding::horizontal(1));
        let line = Line::from(vec![Span::styled(value, v_style), Span::raw(format!(" · {line2}"))]);
        f.render_widget(Paragraph::new(fit_line(line.spans, cell.width.saturating_sub(4) as usize)).block(b), cell);
    }
}

/// Width at which the tidy block keeps MERGED / MAIN CI on the same rows as ISSUES / PRS.
const TIDY_TWO_COL: u16 = 56;

/// The tidy block's stat rows: `ISSUES  12 open   5 new   7 free    MERGED   7 today` /
/// `PRS      1 open   0 failing         MAIN CI  ok` (4 rows when narrower than 56).
fn tidy_stats(app: &App, s: &github::GhSnapshot, width: u16) -> Vec<Line<'static>> {
    let fac = github::factory(s, &app.snap.cards, app.snap.now);
    let left1 = format!(" {:<7}{:>3} open {:>3} new {:>3} free", "ISSUES", s.issues_open, fac.new_today, fac.unclaimed);
    let left2 = format!(" {:<7}{:>3} open {:>3} failing", "PRS", s.prs.len(), fac.failing);
    let right1 = vec![Span::raw(format!("{:<9}{:>2} today", "MERGED", s.merged_today.len()))];
    let main = s.main_ci.as_ref().map(|c| c.state.clone()).unwrap_or_else(|| "-".into());
    let right2 = vec![
        Span::raw(format!("{:<9}", "MAIN CI")),
        Span::styled(main.clone(), if main == "FAIL" { red() } else { Style::default() }),
    ];
    if width >= TIDY_TWO_COL {
        let pad = |l: &str| format!("{l:<36}");
        let mut a = vec![Span::raw(pad(&left1))];
        a.extend(right1);
        let mut b = vec![Span::raw(pad(&left2))];
        b.extend(right2);
        vec![Line::from(a), Line::from(b)]
    } else {
        let indent = |mut v: Vec<Span<'static>>| {
            v.insert(0, Span::raw(" "));
            Line::from(v)
        };
        vec![Line::raw(left1), Line::raw(left2), indent(right1), indent(right2)]
    }
}

/// Tidy rows: kind (6) · #number (7) · title (flex) · status (right-aligned, 8).
/// PRs first (`CI ok|FAIL|run`), then issues (`PR ok|FAIL|run`, the owner, `board`, `free`).
fn tidy_rows(app: &App, s: &github::GhSnapshot, width: usize) -> Vec<Line<'static>> {
    let fac = github::factory(s, &app.snap.cards, app.snap.now);
    let row = |kind: &str, n: i64, title: &str, status: String| {
        let head = format!(" {kind:<6}{:<8}", format!("gh#{n}"));
        let room = width.saturating_sub(head.chars().count() + 8 + 2);
        let mut spans = vec![Span::raw(head), Span::raw(format!("{:<room$} ", fit(title, room)))];
        let status = format!("{:>8}", fit(&status, 8));
        spans.extend(fail_red(&status).spans);
        Line::from(spans)
    };
    let mut out = Vec::new();
    for p in &s.prs {
        out.push(row("PR", p.number, &github::short_title(&p.title), format!("CI {}", p.ci)));
    }
    for r in &fac.issues {
        let status = match r.kind {
            // the 8-char status cell: the linked PR's CI (the row's number is the ISSUE's)
            github::StateKind::Pr => format!("PR {}", r.pr_ci.clone().unwrap_or_else(|| "-".into())),
            github::StateKind::InProgress => r.who.clone(),
            github::StateKind::OnBoard => "board".into(),
            github::StateKind::Unclaimed => "free".into(),
        };
        out.push(row("ISSUE", r.number, &github::short_title(&r.title), status));
    }
    out
}

/// Rows the tidy block needs for its stats (+ the divider).
fn tidy_stats_height(width: u16) -> u16 {
    if width >= TIDY_TWO_COL {
        3
    } else {
        5
    }
}

/// The tidy GitHub block (third-h, third-v, and any compact GitHub rendering):
/// `GITHUB · repo` with the time right-aligned in the border, two stat rows, a divider,
/// then fixed-column PR/issue rows and `+N more … tab >`. Focusable like the full panel.
pub(super) fn draw_gh_compact(f: &mut Frame, app: &App, area: Rect) {
    note_area(app, 0, area);
    let focused = app.focus == Focus::Github;
    let sel_style = bold().add_modifier(Modifier::REVERSED);
    let fg = palette(&app.snap.theme).fg;
    let Some(repo) = app.gh.repo.clone() else {
        let b = frame(focused, None).title(Span::styled(" GITHUB ", bold().fg(fg)));
        let inner = b.inner(area);
        f.render_widget(b, area);
        let st = if focused { sel_style } else { Style::default() };
        f.render_widget(Paragraph::new(Line::styled(" no repo — enter to pick one (or R)", st)), inner);
        return;
    };
    let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
    let (suffix, is_red) = github::sync_suffix(app.gh.error.as_deref(), app.gh.fails);
    let left = if focused && app.gh_sel == 0 {
        Span::styled(fit_title(&["GITHUB", &format!("{name} (enter to change)")], area.width), sel_style)
    } else {
        Span::styled(fit_title(&["GITHUB", &name], area.width), if is_red { red().fg(fg) } else { bold().fg(fg) })
    };
    let mut b = frame(focused, None).title(left.clone());
    if let Some(snap) = &app.gh.snap {
        let t = format!(" {}{suffix} ", crate::store::fmt_clock(snap.fetched_at));
        // the time (and, when failing, the suffix) only when it fits next to the title
        if left.content.chars().count() + t.chars().count() + 4 <= area.width as usize {
            b = b.title(Line::styled(t, if is_red { red() } else { dim() }).right_aligned());
        }
    }
    let inner = b.inner(area);
    f.render_widget(b, area);
    let w = inner.width as usize;
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    let Some(s) = &app.gh.snap else {
        if app.gh.error.is_none() && y < bottom {
            f.render_widget(Paragraph::new(Line::styled(" fetching...", dim())), Rect { y, height: 1, ..inner });
        }
        return;
    };
    // stats (dropped last when there is no room at all), then a divider
    let stats = tidy_stats(app, s, inner.width);
    let stat_rows = (stats.len() as u16).min(bottom.saturating_sub(y));
    f.render_widget(
        Paragraph::new(stats.into_iter().take(stat_rows as usize).map(|l| fit_line(l.spans, w)).collect::<Vec<_>>()),
        Rect { y, height: stat_rows, ..inner },
    );
    y += stat_rows;
    if y < bottom {
        f.render_widget(Paragraph::new(Line::styled(" ".to_string() + &"─".repeat(w.saturating_sub(2)), dim())), Rect { y, height: 1, ..inner });
        y += 1;
    }
    if y >= bottom {
        return;
    }
    draw_tidy_list(f, app, s, Rect { y, height: bottom - y, ..inner }, focused);
}

/// The tidy PR/issue rows in `area` (selection, `+N more … tab >`); also used under the
/// tiles of the full GitHub panel when its wide tables would crush the title.
pub(super) fn draw_tidy_list(f: &mut Frame, app: &App, s: &github::GhSnapshot, area: Rect, focused: bool) {
    let sel_style = bold().add_modifier(Modifier::REVERSED);
    let w = area.width as usize;
    let mut rows = tidy_rows(app, s, w);
    let visible = area.height as usize;
    let sel = (focused && app.gh_sel >= 1).then(|| app.gh_sel - 1);
    if let Some(k) = sel {
        if let Some(l) = rows.get_mut(k) {
            *l = l.clone().patch_style(sel_style);
        }
    }
    let total = rows.len();
    let off = sel.map_or(0, |k| (k + 1).saturating_sub(visible.saturating_sub(1).max(1)));
    let mut shown: Vec<Line> = rows.into_iter().skip(off).take(visible).collect();
    let hidden = total.saturating_sub(off + shown.len());
    if hidden > 0 && visible >= 2 {
        shown.truncate(visible - 1);
        let more = total - off - shown.len();
        let left = format!(" +{more} more");
        let right = "tab > ";
        let pad = w.saturating_sub(left.len() + right.len());
        shown.push(Line::styled(format!("{left}{}{right}", " ".repeat(pad)), dim()));
    }
    if total == 0 {
        shown = vec![Line::styled(" no open PRs or issues", dim())];
    }
    f.render_widget(Paragraph::new(shown), area);
}

/// Rows a tidy block wants: frame + stats + divider + all rows.
fn tidy_full_height(app: &App, width: u16) -> u16 {
    let rows = app.gh.snap.as_ref().filter(|_| app.gh.repo.is_some()).map_or(1, |s| (s.prs.len() + s.issues.len()).max(1) as u16);
    if app.gh.repo.is_none() {
        3
    } else {
        2 + tidy_stats_height(width.saturating_sub(2)) + rows
    }
}

/// Lines for a compact AGENTS panel: `* name status #card "note" age title`, one per actor of
/// this board, then `+N elsewhere`.
///
/// A row too narrow for all of it gives up whole fields, the least useful first: the live
/// status word (the mark already says it), then the note, then the note's age, then the card
/// title (the one field that is cut, never below 4 characters), and the card id last. An idle
/// holder's warning is never the part that goes.
fn agent_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let r = app.roster();
    if r.total() == 0 {
        return vec![crate::tui::agents_empty(app)];
    }
    let room = width.saturating_sub(3);
    let mut lines: Vec<Line<'static>> = r
        .here
        .iter()
        .map(|row| {
            let holds = row.idle_holder();
            let (mark, st) = match (holds, row.live.is_some(), row.status()) {
                (true, _, _) => ("!", red()),
                (_, false, _) => (" ", Style::default()),
                (_, _, "working") => ("*", Style::default().fg(GREEN)),
                (_, _, "blocked") => ("x", bold()),
                _ => ("-", dim()),
            };
            let text = match row.card {
                Some(c) if holds => holder_text(app, row, c, width),
                Some(c) => {
                    // the status word is the first to go: it stays only on a row that has room
                    // for everything else too (the note when there is one, and its age)
                    let with = card_text(app, row, c, room, true);
                    let all = if app.snap.last_note.contains_key(&c.id) { 2 } else { 1 };
                    if with.0 == all { with.1 } else { card_text(app, row, c, room, false).1 }
                }
                None => {
                    // status word first, then the age, then the words (`last_seen`)
                    let name = format!(" {:<10}", fit(&row.name, 10));
                    let with = format!("{name} {:<8} {}", row.status(), crate::tui::last_seen(app, row, usize::MAX));
                    if row.live.is_some() && with.chars().count() <= room {
                        with
                    } else {
                        format!("{name} {}", crate::tui::last_seen(app, row, room.saturating_sub(12)))
                    }
                }
            };
            Line::from(vec![Span::raw(" "), Span::styled(mark, st), Span::raw(fit(text.trim_end(), room))])
        })
        .collect();
    if !r.elsewhere.is_empty() {
        lines.push(crate::tui::elsewhere_line(r.elsewhere.len(), width.saturating_sub(1)));
    }
    if app.focus == Focus::Agents {
        if let Some(l) = lines.get_mut(app.ag_sel) {
            *l = l.clone().patch_style(bold().add_modifier(Modifier::REVERSED));
        }
    }
    lines
}

/// `#4 ` for the card an actor holds, `review #7 ` for the one it reviews: the word travels
/// with the id, the last field a narrow row gives up.
fn card_id_text(row: &crate::roster::Row, c: &Card) -> String {
    if row.role == Some(crate::roster::CardRole::Reviewer) {
        format!("review #{} ", c.id)
    } else {
        format!("#{} ", c.id)
    }
}

/// One card row fitted to `room`, with or without the status word, as (what the activity
/// shows: 2 = note and age, 1 = the age, 0 = nothing; the text).
fn card_text(app: &App, row: &crate::roster::Row, c: &Card, room: usize, status: bool) -> (u8, String) {
    // no herdr pane of this name: there is no status word, and the note gets its columns
    let head = if status && row.live.is_some() { format!(" {:<10} {:<8} ", fit(&row.name, 10), row.status()) } else { format!(" {:<10} ", fit(&row.name, 10)) };
    let id = card_id_text(row, c);
    let age = app.snap.last_event_at.get(&c.id).map(|ts| crate::store::fmt_age((app.snap.now - ts).max(0))).unwrap_or_default();
    // the note gets what the row has left after the name, status, id and a short title; the
    // age is shown even without a note
    let title = format!(" {}", fit(&c.title, 12));
    let left = room.saturating_sub(head.chars().count() + id.chars().count());
    // (one column more than the row has, as ever: the closing cut lands in the title)
    let act = crate::tui::activity(app.snap.last_note.get(&c.id), &age, (left + 1).saturating_sub(title.chars().count()));
    if !act.is_empty() {
        let shows = if act.starts_with('"') { 2 } else { 1 };
        return (shows, format!("{head}{id}{act}{title}"));
    }
    // no room for the activity: the title takes what is left (cut, but never below 4
    // characters); after that only the card id is left
    let whole = c.title.chars().count() <= left;
    (0, if whole || left >= 4 { format!("{head}{id}{}", fit(&c.title, left)) } else { format!("{head}{id}") })
}

/// An idle holder's row: ` name #5 "note" 1h20m title · idle w/ card (1h20m)`. The warning and
/// its duration stay whole; the activity goes first, then the title is shortened.
fn holder_text(app: &App, row: &crate::roster::Row, c: &Card, width: usize) -> String {
    let age = app.snap.last_event_at.get(&c.id).map(|ts| crate::store::fmt_age((app.snap.now - ts).max(0))).unwrap_or_default();
    let id = card_id_text(row, c);
    let title = format!(" {}", fit(&c.title, 12));
    let held = crate::tui::idle_hold_age(app, row);
    let held_for = held.chars().count();
    let tail = 15 + held_for;
    // a row that ends in a duration is budgeted to the column (` name ` is 12), so the
    // duration is never the part that gets cut
    let lead = if held_for > 0 { 12 } else { 11 };
    let fixed = 3 + lead + tail + id.chars().count() + title.chars().count();
    let act = crate::tui::activity(app.snap.last_note.get(&c.id), &age, width.saturating_sub(fixed));
    // what the title may take so that ` · idle w/ card (1h20m)` still fits
    let title_room = width.saturating_sub(3 + 12 + id.chars().count() + tail);
    let what = if act.is_empty() && held_for > 0 && title_room >= 4 {
        // no room for the activity next to the duration: the duration says the same age, so
        // keep the warning whole and shorten the title instead
        format!("{id}{}", fit(&c.title, title_room))
    } else if act.is_empty() {
        format!("{id}{}", c.title)
    } else {
        format!("{id}{act}{title}")
    };
    format!(" {:<10} {} · idle w/ card{held}", fit(&row.name, 10), what)
}

pub(super) fn draw_agents_compact(f: &mut Frame, app: &App, area: Rect) {
    note_area(app, 1, area);
    let r = app.roster();
    // nobody on this board and no agent pane anywhere: the title reads as it always has
    let (a, b) = if r.total() == 0 { ("0 working".to_string(), "0 idle".to_string()) } else { crate::roster::counts(&r) };
    let fg = palette(&app.snap.theme).fg;
    let b = frame(app.focus == Focus::Agents, None).title(Span::styled(fit_title(&["AGENTS", &a, &b], area.width), bold().fg(fg)));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let lines = crate::tui::close_clipped(agent_lines(app, inner.width as usize), app, inner.height as usize, inner.width as usize);
    let off = (app.ag_sel + 1).saturating_sub(inner.height as usize) as u16;
    let off = if app.focus == Focus::Agents { off } else { 0 };
    f.render_widget(Paragraph::new(lines).scroll((off, 0)), inner);
}

/// Rows the AGENTS panel wants: one per actor of this board plus the `+N elsewhere` line.
fn agent_count(app: &App) -> u16 {
    app.roster().panel_rows().max(1) as u16
}

/// RAIL (wide and short, e.g. 126x24 / 200x24): columns on the left, GITHUB above AGENTS
/// on the right, all boxed. A panel only becomes a bar when even 3 rows don't fit.
pub(super) fn draw_rail(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(area);
    f.render_widget(Paragraph::new(header(app, area.width)), rows[0]);
    f.render_widget(Paragraph::new(footer(app, area.width)), rows[2]);
    let (gh_on, ag_on) = (app.show_github, app.show_agents);
    let body = rows[1];
    if !gh_on && !ag_on {
        app.shown.set((false, false));
        app.bars.set((false, false));
        draw_columns(f, app, body);
        return;
    }
    let w = body.width;
    if w < 60 {
        // too narrow for a rail: columns, then a bar per enabled panel
        let n_bars = u16::from(gh_on) + u16::from(ag_on);
        let parts = Layout::vertical([Constraint::Min(0), Constraint::Length(n_bars)]).split(body);
        draw_columns(f, app, parts[0]);
        let mut lines = Vec::new();
        if gh_on {
            lines.push(gh_bar(app, w as usize));
        }
        if ag_on {
            lines.push(ag_bar(app, w as usize));
        }
        f.render_widget(Paragraph::new(lines), parts[1]);
        app.shown.set((false, false));
        app.bars.set((gh_on, ag_on));
        return;
    }
    let rail_w = if w < 130 { (w * 34 / 100).max(36) } else { w * 38 / 100 };
    let parts = Layout::horizontal([Constraint::Min(10), Constraint::Length(rail_w.min(w.saturating_sub(10)))]).split(body);
    draw_columns(f, app, parts[0]);
    let rail = parts[1];
    let h = rail.height;
    // agents: one line each (up to 8); github gets the rest
    let ag_want = if ag_on { agent_count(app).min(8) + 2 } else { 0 };
    let gh_min = if gh_on { 5 } else { 0 };
    let ag_h = if gh_on { ag_want.min(h.saturating_sub(gh_min)).max(if ag_on { 3.min(h) } else { 0 }) } else { h.min(ag_want.max(3)) };
    let mut gh_h = if gh_on { h.saturating_sub(ag_h) } else { 0 };
    let mut ag_h = ag_h;
    if gh_on && app.gh.repo.is_none() && gh_h > 3 {
        // no repo yet: a 3-row "pick one" box; agents get the rest
        if ag_on {
            ag_h += gh_h - 3;
        }
        gh_h = 3;
    }
    let (gh_panel, ag_panel) = (gh_h >= 3, ag_h >= 3);
    let mut y = rail.y;
    let mut bars = (false, false);
    if gh_on {
        if gh_panel {
            draw_gh_compact(f, app, Rect { y, height: gh_h, ..rail });
        } else if gh_h > 0 {
            note_area(app, 0, Rect { y, height: 1, ..rail });
            f.render_widget(Paragraph::new(gh_bar(app, rail.width as usize)), Rect { y, height: 1, ..rail });
            bars.0 = true;
        }
        y += gh_h;
    }
    if ag_on {
        if ag_panel {
            draw_agents_compact(f, app, Rect { y, height: ag_h, ..rail });
        } else {
            note_area(app, 1, Rect { y, height: 1.min(rail.y + h - y), ..rail });
            f.render_widget(Paragraph::new(ag_bar(app, rail.width as usize)), Rect { y, height: 1.min(rail.y + h - y), ..rail });
            bars.1 = true;
        }
    }
    app.shown.set((gh_on && gh_panel, ag_on && ag_panel));
    app.bars.set(bars);
}

/// The four columns side by side (boxed cards, dense when tight).
fn draw_columns(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(area);
    let dense = any_dense(app, &(0..4).map(|ci| (ci, cols[ci])).collect::<Vec<_>>());
    for ci in 0..4 {
        draw_column(f, app, ci, cols[ci], dense);
    }
}

/// STACK (tall and narrow, e.g. 42x73 / 60x73): the columns as stacked boxed sections, then
/// GITHUB, then AGENTS. Panels keep a minimum; card rows yield first (`+N more`).
pub(super) fn draw_stack(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(area);
    let title = format!(" TERMINAL BOARD · {} · {} cards", board_name(app), app.snap.cards.len());
    f.render_widget(Paragraph::new(Line::styled(fit(&title, area.width as usize), bold())), rows[0]);
    f.render_widget(Paragraph::new(footer(app, area.width)), rows[2]);
    let body = rows[1];
    let bh = body.height;
    let (gh_on, ag_on) = (app.show_github, app.show_agents);
    let counts: Vec<u16> = (0..4).map(|c| app.col_cards(c).len() as u16).collect();
    // cards: every section at least 2 dense boxes (or all its cards), ideally all 4-row boxes
    let cards_min: u16 = (0..4).map(|c| column_min_boxed(app, c, body.width)).sum();
    let cards_full: u16 = (0..4).map(|c| if counts[c] == 0 { 1 } else { column_height(app, c, body.width, false) }).sum();
    // GITHUB is capped at ~8 rows (+N more) until every section shows its minimum
    let gh_full = if gh_on { tidy_full_height(app, body.width) } else { 0 };
    let gh_cap = if app.gh.repo.is_some() { gh_full.min(2 + tidy_stats_height(body.width.saturating_sub(2)) + STACK_GH_ROWS) } else { gh_full };
    let ag_full = if ag_on { 2 + agent_count(app).min(10) } else { 0 };
    let ag_min = ag_full.min(4);
    let (mut gh_h, mut ag_h);
    if cards_min + gh_cap + ag_min <= bh {
        gh_h = gh_cap;
        ag_h = ag_min;
        let mut left = bh - cards_min - gh_h - ag_h;
        // cards up to ~55% first, then the rest of GITHUB (issue #4: spare rows must not
        // sit unused between DONE and GITHUB), then AGENTS; whatever remains goes to cards
        let cards_target = (bh * 55 / 100).clamp(cards_min, cards_full.max(cards_min));
        left -= (cards_target - cards_min).min(left);
        let add = (gh_full - gh_h).min(left);
        gh_h += add;
        left -= add;
        ag_h += (ag_full - ag_h).min(left);
    } else {
        // tight: the cards keep their minimum, panels shrink, then become bars (never dropped)
        ag_h = if ag_on { 3 } else { 0 };
        gh_h = if gh_on { bh.saturating_sub(cards_min + ag_h).min(gh_cap) } else { 0 };
        if gh_on && gh_h < 3 {
            gh_h = 1;
        }
        if ag_on && cards_min + gh_h + ag_h > bh {
            ag_h = 1;
        }
    }
    let cards_h = bh.saturating_sub(gh_h + ag_h);
    let cards_area = Rect { height: cards_h, ..body };
    draw_sections(f, app, cards_area, &counts);
    let mut y = body.y + cards_h;
    let mut bars = (false, false);
    if gh_on && gh_h > 0 {
        let r = Rect { y, height: gh_h.min(body.y + bh - y), ..body };
        if gh_h >= 3 {
            draw_gh_compact(f, app, r);
        } else {
            note_area(app, 0, Rect { height: 1, ..r });
            f.render_widget(Paragraph::new(gh_bar(app, body.width as usize)), Rect { height: 1, ..r });
            bars.0 = true;
        }
        y += gh_h;
    }
    if ag_on && ag_h > 0 && y < body.y + bh {
        let r = Rect { y, height: ag_h.min(body.y + bh - y), ..body };
        if ag_h >= 3 {
            draw_agents_compact(f, app, r);
        } else {
            note_area(app, 1, Rect { height: 1, ..r });
            f.render_widget(Paragraph::new(ag_bar(app, body.width as usize)), Rect { height: 1, ..r });
            bars.1 = true;
        }
    }
    app.shown.set((gh_on && gh_h >= 3, ag_on && ag_h >= 3));
    app.bars.set(bars);
}

/// GITHUB issue/PR rows the stack shows before every card section has its minimum.
const STACK_GH_ROWS: u16 = 8;

/// The four columns as stacked sections sharing `area` (selected section first). A section
/// is either a boxed column (at least 2 dense cards, or all of them) or its 1-row header —
/// never the unboxed list — and all boxed sections share one card style.
fn draw_sections(f: &mut Frame, app: &App, area: Rect, counts: &[u16]) {
    let w = area.width;
    let order: Vec<usize> = std::iter::once(app.col).chain((0..4).filter(|c| *c != app.col)).collect();
    let mut heights = [1u16; 4];
    let mut left = area.height.saturating_sub(4);
    let claimants = || (0..4).filter(|c| counts[*c] > 0);

    // ONE CARD EACH, FAIRLY, BEFORE ANYBODY GETS TWO.
    //
    // This pass used to be first-come and all-or-nothing, walking from the SELECTED column:
    // a long selected column took its whole minimum and a later column, unable to afford
    // its own, stayed a bare header — a short column showing nothing while a long one filled
    // the pane, which is the bug this whole change is about. It is the selection that made it
    // unpredictable: the same board looked different depending on where the cursor happened
    // to be. Now nobody may take more than an equal share of what is left while another
    // column still has nothing, so the outcome does not depend on the cursor at all.
    for pass in [0u8, 1] {
        for &c in &order {
            if counts[c] == 0 {
                continue;
            }
            let need = match pass {
                // pass 0: one card each · pass 1: top up toward two, same fairness
                0 if heights[c] > 1 => continue,
                0 => column_min_one(app, c, w) - 1,
                _ => column_min_boxed(app, c, w).saturating_sub(heights[c]),
            };
            if need == 0 {
                continue;
            }
            let waiting = claimants().filter(|x| heights[*x] == 1).count().max(1) as u16;
            let fair = if pass == 0 { left / waiting } else { left };
            // a part-grant would be a frame with nothing in it: a column takes its minimum
            // whole, or stays a header, where the count in the header tells the truth
            if need <= fair {
                heights[c] += need;
                left -= need;
            }
        }
    }
    // Then grow toward all dense boxes, then all 4-row boxes — but a FAIR SHARE at a time.
    //
    // This is the fix for "the done has too many and it pushes everyone": these four sections
    // share one height, and growing each to everything it wanted in turn let the first long
    // column take the lot, leaving the others as one-row headers. Each column may now reach
    // its share of the room before any column takes a second helping; what no column wants is
    // handed out afterwards, so nothing is wasted. Cards that do not fit say `+N more`.
    let wants = |c: usize| counts[c] > 0;
    let sharers = (0..4).filter(|c| wants(*c)).count().max(1) as u16;
    for dense in [true, false] {
        for share in [area.height / sharers, u16::MAX] {
            for &c in &order {
                if counts[c] == 0 {
                    continue;
                }
                // A column the passes above could not afford is rescued here if room came
                // free — to a whole minimum, never an empty frame, and never by taking what
                // another column still waiting for its first card would need. Rescuing on a
                // first-come basis is exactly the unfairness this change is about.
                let floor = if heights[c] == 1 { column_min_one(app, c, w) } else { heights[c] };
                if floor > heights[c] {
                    let stuck = claimants().filter(|x| heights[*x] == 1).count().max(1) as u16;
                    if floor - heights[c] > left / stuck {
                        continue;
                    }
                }
                let want = column_height(app, c, w, dense).max(floor).min(share.max(floor));
                let add = want.saturating_sub(heights[c]).min(left);
                heights[c] += add;
                left -= add;
            }
        }
    }
    // spare height must not sit as a blank band between sections and the panels
    // (issue #4): stretch the LAST boxed section to absorb what is left
    if left > 0 {
        if let Some(&c) = order.iter().rev().find(|&&c| counts[c] > 0 && heights[c] > 1) {
            heights[c] += left;
        }
    }
    let boxed = |c: usize| counts[c] > 0 && heights[c] > 1;
    let mut rects = Vec::new();
    let mut yy = area.y;
    for (ci, h) in heights.iter().enumerate() {
        if boxed(ci) {
            rects.push((ci, Rect { y: yy, height: *h, ..area }));
        }
        yy += h;
    }
    let dense = any_dense(app, &rects);
    let mut y = area.y;
    for ci in 0..4 {
        let h = heights[ci].min((area.y + area.height).saturating_sub(y));
        if h == 0 {
            continue;
        }
        let r = Rect { y, height: h, ..area };
        if !boxed(ci) {
            note_col(app, ci, Rect { height: 1, ..r });
            let col = COLUMNS[ci];
            let colour = column_colour_in(col, &app.snap.theme);
            let count = if col == "doing" { format!("{}/{}", counts[ci], app.snap.wip) } else { counts[ci].to_string() };
            // ` o NAME (count)`: a label gives way in whole words before the count does
            let name = column_name(&app.snap, col, (area.width as usize).saturating_sub(6 + count.len()));
            let l = Line::from(vec![
                Span::styled(" o", Style::default().fg(colour)),
                Span::styled(format!(" {name} ({count})"), bold().fg(colour)),
            ]);
            f.render_widget(Paragraph::new(l), Rect { height: 1, ..r });
        } else {
            draw_column(f, app, ci, r, dense);
        }
        y += h;
    }
}

/// FOCUS (small box, e.g. 50x14): counts, then ONE card big in its column-coloured box —
/// title (wrapped), meta, checklist (cursor), last note — then the GITHUB/AGENTS bars.
pub(super) fn draw_focus(f: &mut Frame, app: &App, area: Rect) {
    app.shown.set((false, false));
    let w = area.width as usize;
    let n = |c: usize| app.col_cards(c).len();
    let plain_counts = format!(" TODO {} · DOING {}/{} · REVIEW {} · DONE {}", n(0), n(1), app.snap.wip, n(2), n(3));
    // with labels: the labelled line when it fits whole, the plain names otherwise
    let lab = |c: usize| app.snap.display.column_label(COLUMNS[c]);
    let labelled = format!(" {} {} · {} {}/{} · {} {} · {} {}", lab(0), n(0), lab(1), n(1), app.snap.wip, lab(2), n(2), lab(3), n(3));
    let counts = if cells(&labelled) <= w { labelled } else { plain_counts };
    let bars_n = u16::from(app.show_github) + u16::from(app.show_agents);
    let bars_n = if area.height >= 6 + bars_n { bars_n } else { 0 };
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(bars_n), Constraint::Length(1)]).split(area);
    f.render_widget(Paragraph::new(Line::raw(fit(&counts, w))), rows[0]);
    f.render_widget(Paragraph::new(footer(app, area.width)), rows[3]);
    let mut bars = Vec::new();
    let mut flags = (false, false);
    if bars_n > 0 && app.show_github {
        note_area(app, 0, Rect { y: rows[2].y, height: 1, ..rows[2] });
        bars.push(gh_bar(app, w));
        flags.0 = true;
    }
    if bars_n > 0 && app.show_agents {
        note_area(app, 1, Rect { y: rows[2].y + u16::from(flags.0), height: 1, ..rows[2] });
        bars.push(ag_bar(app, w));
        flags.1 = true;
    }
    app.bars.set(flags);
    f.render_widget(Paragraph::new(bars), rows[2]);
    let body = rows[1];
    let Some(card) = app.focus_target() else {
        f.render_widget(Paragraph::new(Line::styled(" nothing here yet — press a to add a card", dim())), body);
        return;
    };
    let colour = column_colour_in(&card.column, &app.snap.theme);
    let fg = palette(&app.snap.theme).fg;
    // ` o NAME ` on the left, ` #id ` on the right of the same border
    let id_w = card.id.to_string().len() + 3;
    let col_name = match app.snap.display.label(&card.column) {
        Some(l) => label_words(&l, (body.width as usize).saturating_sub(2 + 5 + id_w)).unwrap_or_else(|| card.column.to_ascii_uppercase()),
        None => card.column.to_ascii_uppercase(),
    };
    let b = frame(true, Some(colour))
        .title(Span::styled(format!(" o {col_name} "), bold().fg(colour)))
        .title(Line::styled(format!(" #{} ", card.id), bold().fg(fg)).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = b.inner(body);
    f.render_widget(b, body);
    let mut lines: Vec<Line> = Vec::new();
    let title = match crate::store::shown_ref(card) {
        Some(n) => format!("{} (gh#{n})", card.title),
        None => card.title.clone(),
    };
    lines.push(Line::styled(title, bold()));
    let parts = meta_parts(card, &app.snap, inner.width as usize);
    let (base, warn, q) = (parts.base, parts.warn, parts.quiet);
    let mut meta = vec![Span::styled(base, dim())];
    if !parts.mark.is_empty() {
        meta.push(Span::raw(" "));
        meta.push(Span::styled(parts.mark, due_mark_style(parts.overdue)));
    }
    if !warn.is_empty() {
        meta.push(Span::raw(" "));
        meta.push(Span::styled(warn, red()));
    }
    // the quiet marker is shown whole or not at all
    let used: usize = meta.iter().map(|s| s.content.chars().count()).sum();
    let lead = usize::from(used > 0);
    if !q.is_empty() && used + lead + q.chars().count() <= inner.width as usize {
        if lead > 0 {
            meta.push(Span::raw(" "));
        }
        meta.push(Span::styled(q, dim()));
    }
    lines.push(Line::from(meta));
    let detail = app.popup.as_ref().filter(|d| d.card.id == card.id).cloned();
    let checklist: Vec<(i64, String, bool)> = match &detail {
        Some(d) => d.checklist.iter().map(|i| (i.idx, i.text.clone(), i.done)).collect(),
        None => app.focus_checklist(card.id),
    };
    if !checklist.is_empty() {
        lines.push(Line::raw(""));
        let cur = app.cursor.min(checklist.len() - 1);
        for (k, (idx, text, done)) in checklist.iter().enumerate() {
            let st = if k == cur && app.focus_nav { bold().add_modifier(Modifier::REVERSED) } else { Style::default() };
            lines.push(Line::styled(format!("[{}] {idx} {text}", if *done { "x" } else { " " }), st));
        }
    }
    if let Some(note) = app.snap.last_note.get(&card.id) {
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("\"{note}\""), Style::default().add_modifier(Modifier::ITALIC)));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// HALF-V grid: rows kept for the grid before panels shrink, and GITHUB's floor.
const GRID_MIN: u16 = 16;
const GRID_GH_MIN: u16 = 14;

/// HALF-V (half the width, tall, e.g. 70x70): the columns as a 2x2 grid (TODO | DOING over
/// REVIEW | DONE), then the GITHUB panel full width (tiles + tables), then AGENTS.
pub(super) fn draw_grid(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(area);
    f.render_widget(Paragraph::new(header(app, area.width)), rows[0]);
    f.render_widget(Paragraph::new(footer(app, area.width)), rows[2]);
    let body = rows[1];
    let bh = body.height;
    let (gh_on, ag_on) = (app.show_github, app.show_agents);
    let halves_w = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(body);
    let cw = [halves_w[0].width, halves_w[1].width];
    // each grid row is as tall as the fuller of its two cells (4-row boxes, else dense)
    let row_need = |r: usize, dense: bool| {
        let h = |ci: usize| column_height(app, ci, cw[ci % 2], dense);
        h(2 * r).max(h(2 * r + 1))
    };
    let full = [row_need(0, false), row_need(1, false)];
    let gh_want = if gh_on { github_want(app, body.width) } else { 0 };
    let ag_want = if ag_on { 2 + agent_count(app).min(8) } else { 0 };
    // the panels' floor: GITHUB >= 14 rows, AGENTS >= 4 (less only when the grid would starve)
    let grid_floor = GRID_MIN.min(bh);
    let ag_min = ag_want.min(4).min(bh.saturating_sub(grid_floor));
    let gh_min = gh_want.min(GRID_GH_MIN).min(bh.saturating_sub(grid_floor + ag_min));
    let cap = bh - gh_min - ag_min;
    let mut grid = if full[0] + full[1] <= cap {
        full
    } else {
        let dense = [row_need(0, true), row_need(1, true)];
        if dense[0] + dense[1] <= cap {
            dense
        } else {
            let top = (u32::from(cap) * u32::from(dense[0]) / u32::from((dense[0] + dense[1]).max(1))) as u16;
            [top, cap - top]
        }
    };
    // spare rows: GITHUB up to what it wants, then AGENTS, then back to the cards
    let mut left = bh - grid[0] - grid[1];
    let mut gh_h = gh_want.min(left.saturating_sub(ag_min));
    left -= gh_h;
    let mut ag_h = ag_want.min(left);
    left -= ag_h;
    for (r, want) in full.iter().enumerate() {
        let add = want.saturating_sub(grid[r]).min(left);
        grid[r] += add;
        left -= add;
    }
    grid[0] += left / 2;
    grid[1] += left - left / 2;
    let mut bars = (false, false);
    if gh_on && gh_h < 5 {
        grid[1] = (grid[1] + gh_h).saturating_sub(1);
        gh_h = 1;
        bars.0 = true;
    }
    if ag_on && ag_h < 3 {
        grid[1] = (grid[1] + ag_h).saturating_sub(1);
        ag_h = 1;
        bars.1 = true;
    }
    let cols_h = grid[0] + grid[1];
    let top = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(Rect { height: grid[0], ..body });
    let bottom = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(Rect { y: body.y + grid[0], height: grid[1], ..body });
    let cells = [top[0], top[1], bottom[0], bottom[1]];
    let dense = any_dense(app, &(0..4).map(|ci| (ci, cells[ci])).collect::<Vec<_>>());
    for (ci, cell) in cells.iter().enumerate() {
        draw_column(f, app, ci, *cell, dense);
    }
    let mut y = body.y + cols_h;
    if gh_on {
        let r = Rect { y, height: gh_h, ..body };
        if bars.0 {
            note_area(app, 0, r);
            f.render_widget(Paragraph::new(gh_bar(app, body.width as usize)), r);
        } else {
            draw_github(f, app, r);
        }
        y += gh_h;
    }
    if ag_on {
        let r = Rect { y, height: ag_h.min(body.y + bh - y), ..body };
        if bars.1 {
            note_area(app, 1, r);
            f.render_widget(Paragraph::new(ag_bar(app, body.width as usize)), r);
        } else {
            draw_agents_compact(f, app, r);
        }
    }
    app.shown.set((gh_on && !bars.0, ag_on && !bars.1));
    app.bars.set(bars);
}

/// A panel full screen (Tab paging): `BOARD · GITHUB · AGENTS` header, the panel, a footer.
pub(super) fn draw_view(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(area);
    let mut tabs = vec![Span::raw(" ")];
    let mut add = |name: &str, on: bool| {
        let st = if on { bold().add_modifier(Modifier::REVERSED) } else { dim() };
        tabs.push(Span::styled(format!(" {name} "), st));
        tabs.push(Span::raw(" · "));
    };
    add("BOARD", false);
    if app.show_github {
        add("GITHUB", app.view == View::Github);
    }
    if app.show_agents {
        add("AGENTS", app.view == View::Agents);
    }
    tabs.pop();
    f.render_widget(Paragraph::new(fit_line(tabs, area.width as usize)), rows[0]);
    match app.view {
        View::Github if rows[1].width >= 100 && rows[1].height >= 14 => draw_github(f, app, rows[1]),
        View::Github => draw_gh_compact(f, app, rows[1]),
        View::Agents => draw_agents_compact(f, app, rows[1]),
        View::Board => {}
    }
    let hints: &[(&str, &str)] = &[("tab", "next"), ("shift+tab", "back"), ("esc", "board"), ("enter", "open"), ("?", "help")];
    f.render_widget(Paragraph::new(Line::from(hint_spans(hints))), rows[2]);
}
