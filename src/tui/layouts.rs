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

/// `AGENTS 5 working · 1 idle (! bot-2 idle w/ card)   tab >`
pub(super) fn ag_bar(app: &App, width: usize) -> Line<'static> {
    let focused = app.focus == Focus::Agents && app.view == View::Board;
    let base = if focused { bold().add_modifier(Modifier::REVERSED) } else { Style::default() };
    let mut spans = vec![Span::styled(" AGENTS ", bold().patch(base))];
    match &app.agents {
        AgentsState::Agents(list) => {
            let working = list.iter().filter(|a| a.status == "working").count();
            let idle = list.iter().filter(|a| a.is_idle()).count();
            spans.push(Span::styled(format!("{working} working · {idle} idle"), base));
            let holders: Vec<String> = list.iter().filter(|a| holds_card(app, a)).map(|a| a.name.clone()).collect();
            if !holders.is_empty() {
                spans.push(Span::styled(" (", base));
                spans.push(Span::styled(format!("! {} idle w/ card", holders.join(", ")), red().patch(base)));
                spans.push(Span::styled(")", base));
            }
        }
        AgentsState::Pending => spans.push(Span::styled("checking herdr...", base)),
        AgentsState::Unavailable(m) => spans.push(Span::styled(m.clone(), base)),
    }
    spans.push(Span::styled(TAB_HINT, dim().patch(base)));
    fit_line(spans, width)
}

/// Cut a line of spans to `width` chars (the last visible span gets an ellipsis).
fn fit_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
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
    let tiles = github::tiles(s, &fac, app.snap.now);
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).split(area);
    for (k, (title, value, line2)) in tiles.into_iter().enumerate() {
        let halves = Layout::horizontal([Constraint::Ratio(1, 2); 2]).spacing(1).split(rows[k / 2]);
        let cell = halves[k % 2];
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
        let head = format!(" {kind:<6}{:<7}", format!("#{n}"));
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
    let left = if focused && app.gh_sel == 0 {
        Span::styled(fit_title(&["GITHUB", &format!("{name} (enter to change)")], area.width), sel_style)
    } else {
        Span::styled(fit_title(&["GITHUB", &name], area.width), bold().fg(fg))
    };
    let mut b = frame(focused, None).title(left.clone());
    if let Some(snap) = &app.gh.snap {
        let t = format!(" {} ", crate::store::fmt_clock(snap.fetched_at));
        // the time only when it fits next to the title
        if left.content.chars().count() + t.len() + 4 <= area.width as usize {
            b = b.title(Line::styled(t, dim()).right_aligned());
        }
    }
    let inner = b.inner(area);
    f.render_widget(b, area);
    let w = inner.width as usize;
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    if let Some(e) = &app.gh.error {
        if y < bottom {
            f.render_widget(Paragraph::new(Line::raw(fit(&format!(" github: {e}"), w))), Rect { y, height: 1, ..inner });
            y += 1;
        }
    }
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

/// Lines for a compact AGENTS panel: `* name status #card title` / job.
fn agent_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let list = match &app.agents {
        AgentsState::Agents(a) => a,
        AgentsState::Pending => return vec![Line::styled(" checking herdr...", dim())],
        AgentsState::Unavailable(m) => return vec![Line::styled(format!(" {m}"), dim())],
    };
    if list.is_empty() {
        return vec![Line::styled(" no agent panes in herdr", dim())];
    }
    list.iter()
        .enumerate()
        .map(|(i, a)| {
            let holds = holds_card(app, a);
            let (mark, st) = match (holds, a.status.as_str()) {
                (true, _) => ("!", red()),
                (_, "working") => ("*", Style::default().fg(GREEN)),
                (_, "blocked") => ("x", bold()),
                _ => ("-", dim()),
            };
            let what = match app.agent_card(i) {
                Some(c) => {
                    let age = app
                        .snap
                        .last_event_at
                        .get(&c.id)
                        .map(|ts| crate::store::fmt_age((app.snap.now - ts).max(0)))
                        .unwrap_or_default();
                    // the note gets what the row has left after the name, status, id and a
                    // short title; the age is shown even without a note
                    let id = format!("#{} ", c.id);
                    let title = format!(" {}", fit(&c.title, 12));
                    let fixed = 3 + 11 + if holds { 15 } else { 9 } + id.chars().count() + title.chars().count();
                    let act = crate::tui::activity(app.snap.last_note.get(&c.id), &age, width.saturating_sub(fixed));
                    if act.is_empty() {
                        format!("{id}{}", c.title)
                    } else {
                        format!("{id}{act}{title}")
                    }
                }
                None => a.job.clone().unwrap_or_else(|| "-".into()),
            };
            let text = if holds {
                format!(" {:<10} {} · idle w/ card", fit(&a.name, 10), what)
            } else {
                format!(" {:<10} {:<8} {}", fit(&a.name, 10), a.status, what)
            };
            let mut l = Line::from(vec![Span::raw(" "), Span::styled(mark, st), Span::raw(fit(&text, width.saturating_sub(3)))]);
            if app.focus == Focus::Agents && app.ag_sel == i {
                l = l.patch_style(bold().add_modifier(Modifier::REVERSED));
            }
            l
        })
        .collect()
}

pub(super) fn draw_agents_compact(f: &mut Frame, app: &App, area: Rect) {
    note_area(app, 1, area);
    let list = agent_list(app);
    let working = list.iter().filter(|a| a.status == "working").count();
    let idle = list.iter().filter(|a| a.is_idle()).count();
    let fg = palette(&app.snap.theme).fg;
    let b = frame(app.focus == Focus::Agents, None)
        .title(Span::styled(fit_title(&["AGENTS", &format!("{working} working"), &format!("{idle} idle")], area.width), bold().fg(fg)));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let lines = agent_lines(app, inner.width as usize);
    let off = (app.ag_sel + 1).saturating_sub(inner.height as usize) as u16;
    let off = if app.focus == Focus::Agents { off } else { 0 };
    f.render_widget(Paragraph::new(lines).scroll((off, 0)), inner);
}

fn agent_count(app: &App) -> u16 {
    match &app.agents {
        AgentsState::Agents(a) if !a.is_empty() => a.len() as u16,
        _ => 1,
    }
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
        // cards up to ~55% first, then AGENTS, then the rest of GITHUB; leftovers to cards
        let cards_target = (bh * 55 / 100).clamp(cards_min, cards_full.max(cards_min));
        left -= (cards_target - cards_min).min(left);
        let add = (ag_full - ag_h).min(left);
        ag_h += add;
        left -= add;
        gh_h += (gh_full - gh_h).min(left);
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
    // boxed minimum, selected section first; a section that can't get it stays a header
    for &c in &order {
        if counts[c] == 0 {
            continue;
        }
        let add = column_min_boxed(app, c, w) - 1;
        if add <= left {
            heights[c] += add;
            left -= add;
        }
    }
    // then grow toward all dense boxes, then all 4-row boxes
    for dense in [true, false] {
        for &c in &order {
            if counts[c] == 0 || heights[c] == 1 {
                continue;
            }
            let add = column_height(app, c, w, dense).saturating_sub(heights[c]).min(left);
            heights[c] += add;
            left -= add;
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
            let name = if col == "done" { "DONE today".to_string() } else { col.to_ascii_uppercase() };
            let count = if col == "doing" { format!("{}/{}", counts[ci], app.snap.wip) } else { counts[ci].to_string() };
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
    let counts = format!(" TODO {} · DOING {}/{} · REVIEW {} · DONE {}", n(0), n(1), app.snap.wip, n(2), n(3));
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
    let col_name = card.column.to_ascii_uppercase();
    let b = frame(true, Some(colour))
        .title(Span::styled(format!(" o {col_name} "), bold().fg(colour)))
        .title(Line::styled(format!(" #{} ", card.id), bold().fg(fg)).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = b.inner(body);
    f.render_widget(b, body);
    let mut lines: Vec<Line> = Vec::new();
    let title = match card.gh_ref {
        Some(n) => format!("{} (gh#{n})", card.title),
        None => card.title.clone(),
    };
    lines.push(Line::styled(title, bold()));
    let (base, warn) = meta_fit(card, &app.snap, inner.width as usize);
    let mut meta = vec![Span::styled(base, dim())];
    if !warn.is_empty() {
        meta.push(Span::raw(" "));
        meta.push(Span::styled(warn, red()));
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
