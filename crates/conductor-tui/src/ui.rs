//! Drawing. One function per screen, all reading [`App`] and never changing it.

use crate::app::{App, Screen};
use crate::theme::Theme;
use conductor_model::{Place, Verdict};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Padding, Paragraph, Row, Table, TableState, Wrap,
};

pub fn draw(f: &mut Frame, app: &App, t: &Theme) {
    let [tabs, body, keys] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_tabs(f, tabs, app, t);
    let body = body.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    match app.screen {
        Screen::Runs => runs(f, body, app, t),
        Screen::Live => live(f, body, app, t),
        Screen::Receipt => receipt(f, body, app, t),
        Screen::Launch => launch(f, body, app, t),
    }
    draw_keys(f, keys, app, t);
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let mut spans = vec![Span::raw(" ")];
    for (i, s) in Screen::ALL.iter().enumerate() {
        let on = *s == app.screen;
        spans.push(Span::styled(format!(" {} ", i + 1), t.faint()));
        let style = if on {
            t.bold().add_modifier(Modifier::UNDERLINED)
        } else {
            t.dim()
        };
        spans.push(Span::styled(s.title(), style));
        spans.push(Span::raw("  "));
    }
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(32)]).areas(area);
    f.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(t.fg(t.line)),
        ),
        left,
    );
    f.render_widget(
        Paragraph::new(Line::from(if app.demo {
            vec![
                Span::styled("demo · ", t.faint()),
                Span::styled("herdr ", t.faint()),
                Span::styled("✓", t.fg(t.pass)),
                Span::styled(" · traces → Tempo ", t.faint()),
            ]
        } else {
            let running = app
                .runs
                .iter()
                .filter(|r| r.status == Verdict::Running)
                .count();
            vec![Span::styled(
                format!("{running} running · {} recorded ", app.receipts.len()),
                t.faint(),
            )]
        }))
        .right_aligned()
        .block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(t.fg(t.line)),
        ),
        right,
    );
}

fn draw_keys(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let mut spans = vec![Span::raw(" ")];
    if let Some(msg) = &app.status {
        spans.push(Span::styled(msg.clone(), t.fg(t.accent)));
    } else {
        for (k, what, hot) in app.keys() {
            let ks = if hot {
                t.fg(t.accent).add_modifier(Modifier::BOLD)
            } else {
                t.text().add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(format!(" {k} "), ks.bg(t.sel)));
            spans.push(Span::styled(
                format!(" {what}   "),
                if hot { t.text() } else { t.dim() },
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A rounded, titled block in the ratatui style the mockup imitates.
fn block<'a>(t: &Theme, title: &'a str, note: &'a str, hot: bool) -> Block<'a> {
    let border = if hot { t.fg(t.accent) } else { t.fg(t.line) };
    let mut title_spans = vec![Span::styled(
        format!(" {title} "),
        if hot {
            t.fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            t.bold()
        },
    )];
    if !note.is_empty() {
        title_spans.push(Span::styled(format!("· {note} "), t.dim()));
    }
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Line::from(title_spans))
        .padding(Padding::new(2, 2, 1, 0))
}

fn lead<'a>(t: &Theme, parts: &[(&'a str, bool)]) -> Paragraph<'a> {
    Paragraph::new(Line::from(
        parts
            .iter()
            .map(|(s, strong)| Span::styled(*s, if *strong { t.text() } else { t.dim() }))
            .collect::<Vec<_>>(),
    ))
    .wrap(Wrap { trim: true })
}

fn verdict_span(t: &Theme, v: Verdict) -> Span<'static> {
    Span::styled(format!("{} {}", v.glyph(), v.word()), t.verdict(v))
}

fn bar(t: &Theme, checks: &[Verdict]) -> Line<'static> {
    Line::from(
        checks
            .iter()
            .map(|v| {
                Span::styled(
                    "■ ",
                    if *v == Verdict::Pending {
                        t.fg(t.line)
                    } else {
                        t.verdict(*v)
                    },
                )
            })
            .collect::<Vec<_>>(),
    )
}

// ── runs ────────────────────────────────────────────────────────────────────

fn runs(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let needs = app.needs_you.clone();
    let [intro, _, stats, _, banner, list, folded, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(if needs.is_some() { 3 } else { 0 }),
        Constraint::Length(app.runs.len() as u16 + 6),
        Constraint::Length(if app.unchecked.is_empty() { 0 } else { 1 }),
        Constraint::Min(0),
    ])
    .areas(area);

    f.render_widget(
        lead(
            t,
            &[
                (
                    "Every run gets its own herdr tab while it's working. ",
                    false,
                ),
                ("Enter", true),
                (" opens a run; ", false),
                ("r", true),
                (" opens its receipt.", false),
            ],
        ),
        intro,
    );

    let stat = |value: &str, label: &str| {
        Text::from(vec![
            Line::from(Span::styled(value.to_owned(), t.bold())),
            Line::from(Span::styled(label.to_owned(), t.dim())),
        ])
    };
    let [a, b, c] = Layout::horizontal([
        Constraint::Length(24),
        Constraint::Length(46),
        Constraint::Min(0),
    ])
    .areas(stats);
    for (area, (value, label)) in [a, b, c].into_iter().zip(app.stats.iter()) {
        f.render_widget(Paragraph::new(stat(value, label)), area);
    }

    if let Some((work, ask, tab)) = needs {
        let line = Line::from(vec![
            Span::styled(
                " ◆ needs you ",
                t.fg(t.blocked).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {work}: {ask}  "), t.text()),
            Span::styled(
                format!("answer in tab {tab}"),
                t.fg(t.accent).add_modifier(Modifier::UNDERLINED),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line).block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(t.fg(t.blocked))
                    .style(Style::new().bg(t.blocked_bg)),
            ),
            banner,
        );
    }

    let header = Row::new(
        ["WORK", "STATUS", "CHECKS", "WHERE", "RECEIPT"]
            .map(|h| Cell::from(Span::styled(h, t.faint()))),
    )
    .bottom_margin(1);
    let rows = app.runs.iter().map(|r| {
        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::styled(r.work.clone(), t.text()),
                Span::styled(format!("  {}", r.run_id), t.faint()),
            ])),
            Cell::from(verdict_span(t, r.status)),
            Cell::from(bar(t, &r.checks)),
            Cell::from(Span::styled(
                r.place.describe(),
                if matches!(r.place, Place::HerdrTab(_)) {
                    t.text()
                } else {
                    t.dim()
                },
            )),
            Cell::from(Span::styled(
                if r.is_finished() { "open" } else { "so far" },
                t.fg(t.accent),
            )),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(34),
            Constraint::Length(13),
            Constraint::Length(11),
            Constraint::Length(15),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .column_spacing(2)
    .row_highlight_style(Style::new().bg(t.sel))
    .highlight_symbol(Span::styled("▌", t.fg(t.accent)))
    .block(block(t, "runs", "newest first", false));
    let mut state = TableState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(table, list, &mut state);
    if let Some(latest) = app.unchecked.first() {
        let n = app.unchecked.len();
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    "  + {n} run{} that never reached a check · conductor receipt {latest}",
                    if n == 1 { "" } else { "s" }
                ),
                t.faint(),
            ))),
            folded,
        );
    }
}

// ── live ────────────────────────────────────────────────────────────────────

fn live(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let run = &app.live;
    // Waiting on a decision: one screen with what the decision needs, and the decision.
    if let (Some(w), Some(r)) = (&run.waiting, &app.review)
        && w.ask.is_none()
    {
        review_screen(f, area, app, t, r);
        return;
    }
    let done = app.live_done();
    let [head, _, pipe, _, ready, wait, cols, _, panes, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(if done { 3 } else { 0 }),
        Constraint::Length(match (&run.waiting, &app.deciding) {
            (Some(_), Some(_)) => 6,
            (Some(_), None) => 5,
            (None, _) => 0,
        }),
        Constraint::Length(10),
        Constraint::Length(1),
        Constraint::Length(if run.herdr_tab.is_some() { 5 } else { 0 }),
        Constraint::Min(0),
    ])
    .areas(area);

    let [h1, h2] = Layout::horizontal([Constraint::Min(0), Constraint::Length(44)]).areas(head);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(run.work.clone(), t.bold()),
            Span::styled(format!("   {} · {}", run.run_id, run.kind), t.faint()),
        ])),
        h1,
    );
    let mut keys = match run.herdr_tab {
        Some(n) => vec![
            Span::styled(" t ", t.bold().bg(t.sel)),
            Span::styled(format!(" watch in tab {n}   "), t.dim()),
        ],
        None => vec![Span::styled("headless   ", t.faint())],
    };
    keys.push(Span::styled(
        " r ",
        t.fg(t.accent).add_modifier(Modifier::BOLD).bg(t.sel),
    ));
    keys.push(Span::styled(" receipt", t.text()));
    f.render_widget(Paragraph::new(Line::from(keys)).right_aligned(), h2);

    // The pipeline: one column per stage, name on top, detail below.
    let widths: Vec<Constraint> = run.stages.iter().map(|_| Constraint::Fill(1)).collect();
    let cells = Layout::horizontal(widths).spacing(2).split(pipe);
    for (i, (s, cell)) in run.stages.iter().zip(cells.iter()).enumerate() {
        let arrow = if i + 1 < run.stages.len() {
            "  →"
        } else {
            ""
        };
        let name_style = if s.status == Verdict::Running {
            t.fg(t.run).add_modifier(Modifier::BOLD)
        } else {
            t.bold()
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(format!("{} ", s.status.glyph()), t.verdict(s.status)),
                    Span::styled(s.name.clone(), name_style),
                    Span::styled(arrow, t.faint()),
                ]),
                Line::from(Span::styled(format!("  {}", s.detail), t.dim())),
            ]),
            *cell,
        );
    }

    if done {
        let ended = run.ended.unwrap_or(Verdict::Passed);
        let (headline, color, bg) = if ended == Verdict::Passed {
            (" ✓ every check passed ".to_string(), t.pass, t.pass_bg)
        } else {
            (
                format!(" {} the run {} ", ended.glyph(), ended.word()),
                t.verdict(ended).fg.unwrap_or(t.fail),
                t.sel,
            )
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(headline, t.fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(" The receipt is ready. ", t.text()),
                Span::styled("⏎ or r to open it", t.fg(t.accent)),
            ]))
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(t.fg(color))
                    .style(Style::new().bg(bg)),
            ),
            ready,
        );
    }

    // The run is paused for a person: what they are asked, and the keys that answer.
    if let Some(w) = &run.waiting {
        waiting_box(f, wait, app, t, w);
    }

    let [checks, log] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(2)
        .areas(cols);
    live_columns(f, checks, log, run, t);
    live_panes(f, panes, run, t);
}

/// The box a person answers from: who is waited for, what is asked, the keys, and the note
/// being typed when there is one.
fn waiting_box(
    f: &mut Frame,
    wait: Rect,
    app: &App,
    t: &Theme,
    w: &conductor_model::view::Waiting,
) {
    {
        let question: String = w.question.split_whitespace().collect::<Vec<_>>().join(" ");
        f.render_widget(
            Paragraph::new(
                vec![
                    Line::from(vec![
                        Span::styled(format!(" waiting for {} ", w.who), t.bold().bg(t.sel)),
                        Span::styled(format!("  stage {}   ", w.stage), t.dim()),
                        Span::styled(" y ", t.fg(t.pass).add_modifier(Modifier::BOLD).bg(t.sel)),
                        Span::styled(
                            if w.ask.is_some() {
                                " allow   "
                            } else {
                                " approve   "
                            },
                            t.text(),
                        ),
                        Span::styled(" n ", t.fg(t.fail).add_modifier(Modifier::BOLD).bg(t.sel)),
                        Span::styled(if w.ask.is_some() { " deny" } else { " reject" }, t.text()),
                    ]),
                    Line::from(Span::styled(question, t.text())),
                ]
                .into_iter()
                .chain(app.deciding.as_ref().map(|(yes, note)| {
                    Line::from(vec![
                        Span::styled(
                            format!(" {} ", if *yes { "approving" } else { "rejecting" }),
                            t.bold().bg(t.sel),
                        ),
                        Span::styled("  note: ", t.dim()),
                        Span::styled(format!("{note}▏"), t.text()),
                        Span::styled("   Enter records · Esc drops", t.dim()),
                    ])
                }))
                .collect::<Vec<_>>(),
            )
            .wrap(Wrap { trim: true })
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(t.fg(t.run)),
            ),
            wait,
        );
    }
}

fn live_columns(
    f: &mut Frame,
    checks: Rect,
    log: Rect,
    run: &conductor_model::view::LiveRun,
    t: &Theme,
) {
    let mut lines: Vec<Line> = run
        .checks
        .iter()
        .map(|c| {
            let mut spans = vec![
                Span::styled(format!("{}  ", c.status.glyph()), t.verdict(c.status)),
                Span::styled(format!("{:<10}", c.name), t.text()),
                Span::styled(c.detail.clone(), t.dim()),
            ];
            if let Some(p) = c.progress {
                let filled = (p as usize * 12) / 100;
                spans.push(Span::styled(
                    format!("  {}", "━".repeat(filled)),
                    t.fg(t.run),
                ));
                spans.push(Span::styled("━".repeat(12 - filled), t.fg(t.line)));
            }
            Line::from(spans)
        })
        .collect();
    if let Some(note) = &run.note {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(note.clone(), t.dim())));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(
                t,
                "checks",
                "run by conductor, not by the agent",
                true,
            )),
        checks,
    );

    let log_lines: Vec<Line> = run
        .log
        .iter()
        .map(|l| {
            Line::from(vec![
                Span::styled(format!("{}  ", l.at), t.faint()),
                Span::styled(format!("{:<11}", l.source.label()), t.source(l.source)),
                Span::styled(l.text.clone(), t.text()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(log_lines).block(block(t, "evidence", "latest", false)),
        log,
    );
}

fn live_panes(f: &mut Frame, panes: Rect, run: &conductor_model::view::LiveRun, t: &Theme) {
    let mut pane_spans = vec![];
    for p in &run.panes {
        let (word, style) = if p.open {
            ("open", t.fg(t.run))
        } else {
            ("closed", t.faint())
        };
        pane_spans.push(Span::styled(
            format!("{} ", p.status.glyph()),
            t.verdict(p.status),
        ));
        pane_spans.push(Span::styled(format!("{} ", p.stage), t.text()));
        pane_spans.push(Span::styled(format!("pane {word}"), style));
        pane_spans.push(Span::raw("      "));
    }
    let title = format!("herdr tab {}", run.herdr_tab.unwrap_or_default());
    f.render_widget(
        Paragraph::new(vec![
            Line::from(pane_spans),
            Line::from(Span::styled(
                "conductor opens a pane per stage and closes it once the stage passes",
                t.faint(),
            )),
        ])
        .block(block(t, &title, "", false)),
        panes,
    );
}

// ── review: one screen to decide from ───────────────────────────────────────

/// The run paused for a decision: the ticket's intent, what changed, what the checks say
/// before and after, what it cost, then the decision. Nothing else; the log, the diff and
/// the receipt stay a key away.
fn review_screen(
    f: &mut Frame,
    area: Rect,
    app: &App,
    t: &Theme,
    r: &conductor_model::view::Review,
) {
    let Some(w) = &app.live.waiting else { return };
    // A box shows one blank row above its rows, so each gets rows + 3.
    let n_criteria = r.criteria.len().max(1) as u16;
    let n_files = r.change.files.len().min(4) as u16 + u16::from(r.change.files.len() > 4);
    let n_checks = r
        .evidence
        .iter()
        .map(|e| 1 + e.lines.len().min(4) as u16)
        .sum::<u16>()
        .max(1);
    let [head, _, asks, _, changed, _, checks, _, cost, _, wait, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(n_criteria + 3),
        Constraint::Length(1),
        Constraint::Length(1 + n_files + 3),
        Constraint::Length(1),
        Constraint::Length(n_checks + 3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(if app.deciding.is_some() { 6 } else { 5 }),
        Constraint::Min(0),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" decide: {} ", r.title), t.bold().bg(t.sel)),
            Span::styled(format!("  {} · waiting for {}", r.run_id, r.who), t.dim()),
        ])),
        head,
    );

    let asks_lines: Vec<Line> = if r.criteria.is_empty() {
        vec![Line::from(Span::styled(
            "the ticket lists no acceptance criteria",
            t.dim(),
        ))]
    } else {
        r.criteria
            .iter()
            .map(|c| {
                Line::from(vec![
                    Span::styled("• ", t.faint()),
                    Span::styled(c.clone(), t.text()),
                ])
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(asks_lines)
            .wrap(Wrap { trim: true })
            .block(block(t, "the ticket asks", "", false)),
        asks,
    );

    let c = &r.change;
    let mut change_lines = vec![Line::from(vec![
        Span::styled(format!("+{} ", c.added), t.fg(t.pass)),
        Span::styled(format!("−{} ", c.removed), t.fg(t.fail)),
        Span::styled(
            format!(
                "in {} file{} · branch {}",
                c.files.len(),
                if c.files.len() == 1 { "" } else { "s" },
                c.branch
            ),
            t.dim(),
        ),
    ])];
    for f_ in c.files.iter().take(4) {
        change_lines.push(Line::from(Span::styled(format!("  {f_}"), t.text())));
    }
    if c.files.len() > 4 {
        change_lines.push(Line::from(Span::styled(
            format!("  +{} more", c.files.len() - 4),
            t.faint(),
        )));
    }
    f.render_widget(
        Paragraph::new(change_lines).block(block(
            t,
            "what changed",
            "the diff is on the branch",
            false,
        )),
        changed,
    );

    let mut check_lines: Vec<Line> = Vec::new();
    for e in &r.evidence {
        let mut spans = vec![Span::styled(
            format!("{}  ", e.after.glyph()),
            t.verdict(e.after),
        )];
        if let Some(b) = e.before {
            spans.push(Span::styled(
                format!("{} before → {} after   ", b.word(), e.after.word()),
                if b != e.after { t.bold() } else { t.dim() },
            ));
        }
        let what = if e.claim.is_empty() {
            e.check.clone()
        } else {
            e.claim.clone()
        };
        spans.push(Span::styled(what, t.text()));
        check_lines.push(Line::from(spans));
        for l in e.lines.iter().take(4) {
            let style = if l.starts_with("FAIL") || l.starts_with('✗') {
                t.fg(t.fail)
            } else {
                t.dim()
            };
            check_lines.push(Line::from(Span::styled(format!("      {l}"), style)));
        }
    }
    if check_lines.is_empty() {
        check_lines.push(Line::from(Span::styled("no check ran", t.dim())));
    }
    f.render_widget(
        Paragraph::new(check_lines)
            .wrap(Wrap { trim: false })
            .block(block(
                t,
                "what the checks say",
                "run by conductor, before the fix and after",
                false,
            )),
        checks,
    );

    let k = &r.cost;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" cost ", t.bold().bg(t.sel)),
            Span::styled(
                format!(
                    "  {} tr{} · {} tokens · ${:.2} · waited on people {}s · {}m{:02}s so far",
                    k.tries,
                    if k.tries == 1 { "y" } else { "ies" },
                    k.tokens,
                    k.cost_usd,
                    k.waited_s,
                    k.wall_s / 60,
                    k.wall_s % 60
                ),
                t.text(),
            ),
        ])),
        cost,
    );

    waiting_box(f, wait, app, t, w);
}

// ── receipt ─────────────────────────────────────────────────────────────────

fn receipt(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let r = &app.receipt;
    let v = r.verdict();
    let how_height = if app.show_how {
        r.how.len() as u16 + 5
    } else {
        1
    };
    // A panel is drawn only when it has rows. An empty one says nothing a box can fix.
    let nothing_checked = r.checks.is_empty();
    let pair_rows = r.survivors.len().max(r.not_checked.len()).min(4) as u16;
    let [banner, _, proven, _, pair, _, how, reach] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(if nothing_checked {
            1
        } else {
            r.checks.len() as u16 + 3
        }),
        Constraint::Length(1),
        Constraint::Length(if pair_rows == 0 { 0 } else { pair_rows + 3 }),
        Constraint::Length(if pair_rows == 0 { 0 } else { 1 }),
        Constraint::Length(how_height),
        Constraint::Length(1),
    ])
    .areas(area);

    let bg = if v == Verdict::Passed {
        t.pass_bg
    } else {
        t.blocked_bg
    };
    let verdict_word = if nothing_checked {
        "NOTHING CHECKED".to_string()
    } else {
        v.word().to_uppercase()
    };
    let counts = if nothing_checked {
        String::new()
    } else {
        format!("{} of {} checks", r.passed_count(), r.checks.len())
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {verdict_word} "),
                t.verdict(v).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {} · {}   ", r.work, r.run_id), t.text()),
            Span::styled(counts, t.fg(t.pass)),
            Span::styled(
                format!("   {} things not checked", r.not_checked.len()),
                t.fg(t.warn),
            ),
        ]))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(t.verdict(v))
                .style(Style::new().bg(bg)),
        ),
        banner,
    );

    let claim_w = r
        .checks
        .iter()
        .map(|c| c.claim.chars().count())
        .max()
        .unwrap_or(0);
    let rows: Vec<Line> = r
        .checks
        .iter()
        .map(|c| {
            Line::from(vec![
                Span::styled(format!("{}  ", c.verdict.glyph()), t.verdict(c.verdict)),
                Span::styled(format!("{:<claim_w$}", c.claim), t.bold()),
                Span::styled(format!("   {}", c.detail), t.dim()),
            ])
        })
        .collect();
    if nothing_checked {
        // Why is in "not checked" below: a halted setup, an agent that never started.
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  nothing was proven: ", t.bold()),
                Span::styled("no check ran", t.dim()),
            ])),
            proven,
        );
    } else {
        f.render_widget(
            Paragraph::new(rows).block(block(
                t,
                "what was proven",
                "each of these could have failed",
                false,
            )),
            proven,
        );
    }

    // Survivors on the left, gaps on the right; one of them alone takes the width.
    let (look, gaps) = match (r.survivors.is_empty(), r.not_checked.is_empty()) {
        (false, false) => {
            let [a, b] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
                .spacing(2)
                .areas(pair);
            (Some(a), Some(b))
        }
        (false, true) => (Some(pair), None),
        (true, false) => (None, Some(pair)),
        (true, true) => (None, None),
    };
    let mut s_lines: Vec<Line> = r
        .survivors
        .iter()
        .take(2)
        .map(|s| {
            Line::from(vec![
                Span::styled(format!("{:<15}", s.at), t.dim()),
                Span::styled(s.change.clone(), t.text()),
            ])
        })
        .collect();
    if r.survivors.len() > 2 {
        s_lines.push(Line::from(Span::styled(
            format!("+{} more", r.survivors.len() - 2),
            t.fg(t.accent),
        )));
    }
    if let Some(look) = look {
        f.render_widget(
            Paragraph::new(s_lines)
                .wrap(Wrap { trim: false })
                .block(block(t, "look here first", "bugs the tests missed", true)),
            look,
        );
    }
    let n_lines: Vec<Line> = r
        .not_checked
        .iter()
        .map(|n| {
            Line::from(vec![
                Span::styled("!  ", t.fg(t.warn)),
                Span::styled(n.clone(), t.text()),
            ])
        })
        .collect();
    if let Some(gaps) = gaps {
        f.render_widget(
            Paragraph::new(n_lines)
                .wrap(Wrap { trim: false })
                .block(block(t, "not checked", "", false)),
            gaps,
        );
    }

    if app.show_how {
        let mut lines: Vec<Line> = r
            .how
            .iter()
            .map(|(k, v)| {
                Line::from(vec![
                    Span::styled(format!("{k:<12}"), t.dim()),
                    Span::styled(v.clone(), t.text()),
                ])
            })
            .collect();
        let i = &r.integrity;
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12}", "record"), t.dim()),
            Span::styled(
                format!(
                    "anchored in {} · signed {} · re-checks cleanly {}",
                    i.anchored_in.as_deref().unwrap_or("—"),
                    if i.signed { "✓" } else { "✗" },
                    if i.reproduces { "✓" } else { "✗" }
                ),
                t.text(),
            ),
        ]));
        f.render_widget(
            Paragraph::new(lines).block(block(t, "how it ran", "h to hide", false)),
            how,
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▸ ", t.fg(t.accent)),
                Span::styled("How it ran, and proof it wasn't changed", t.text()),
                Span::styled("   h", t.faint()),
            ])),
            how,
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("Also at: {}", r.also_at.join("  ·  ")),
            t.dim(),
        ))),
        reach,
    );
}

// ── launch ──────────────────────────────────────────────────────────────────

fn launch(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let l = &app.launch;
    let radio = |on: bool, label: &str, later: Option<&str>| -> Vec<Span<'static>> {
        let mut v = vec![
            Span::styled(
                if on { "(•) " } else { "( ) " },
                if on { t.fg(t.accent) } else { t.faint() },
            ),
            Span::styled(
                label.to_owned(),
                if later.is_some() { t.faint() } else { t.text() },
            ),
        ];
        if let Some(r) = later {
            v.push(Span::styled(format!(" {r}"), t.faint()));
        }
        v.push(Span::raw("     "));
        v
    };
    let q = |n: usize, text: &str| -> Line<'static> {
        let focused = l.focus + 1 == n;
        Line::from(vec![
            Span::styled(if focused { "▸ " } else { "  " }, t.fg(t.accent)),
            Span::styled(
                format!("{n}  "),
                t.fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(text.to_owned(), if focused { t.bold() } else { t.text() }),
        ])
    };

    let mut lines = vec![
        Line::from(Span::styled(
            "Three questions, then conductor opens a new herdr tab for the run.",
            t.dim(),
        )),
        Line::raw(""),
    ];
    lines.push(q(1, "What kind of work?"));
    let mut kinds = vec![Span::raw("     ")];
    kinds.extend(radio(true, "build a change", None));
    kinds.extend(radio(false, "ask a question", Some("v0.2")));
    kinds.extend(radio(false, "QA", Some("v0.2")));
    kinds.extend(radio(false, "troubleshoot", Some("v0.3")));
    lines.push(Line::from(kinds));
    lines.push(Line::from(Span::styled(
        "     spec → tests by one agent → code by another",
        t.dim(),
    )));
    lines.push(Line::raw(""));
    lines.push(q(2, "What should it build?"));
    let cursor = if l.focus == 1 { "▏" } else { "" };
    lines.push(Line::from(vec![
        Span::raw("     "),
        Span::styled(format!(" {}{cursor} ", l.spec), t.text().bg(t.sel)),
    ]));
    lines.push(Line::raw(""));
    lines.push(q(3, "Where should it run?"));
    let mut place = vec![Span::raw("     ")];
    place.extend(radio(!l.headless, "herdr, in a new tab", None));
    place.extend(radio(l.headless, "in the background", None));
    lines.push(Line::from(place));
    lines.push(Line::from(Span::styled(
        if l.headless {
            "     no panes; for CI and overnight"
        } else {
            "     you can watch and step in · opens herdr tab 5; 3 runs already active"
        },
        t.dim(),
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::raw("     "),
        Span::styled(
            " Start run ⏎ ",
            t.fg(t.sel).bg(t.accent).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("     Defaults from this repo's policy: tests must catch 70% of injected bugs · 2 test runs · 60 min budget", t.faint())));

    let height = (lines.len() as u16 + 3).min(area.height);
    let [form, _] = Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(
                t,
                "new run",
                "the workflow and policy are read from the base commit",
                true,
            )),
        form,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use conductor_model::view::LiveRun;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app, &Theme::DARK)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn the_runs_screen_shows_the_list_and_who_needs_you() {
        let app = App::demo();
        let s = render(&app, 140, 40);
        for want in [
            "PTT-1234 add --json to status",
            "needs you",
            "herdr tab 2",
            "headless · CI",
            "so far",
            "receipt",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
    }

    #[test]
    fn a_receipt_with_nothing_checked_says_so_in_one_line_and_draws_no_empty_box() {
        let mut app = App::demo();
        app.receipt.checks.clear();
        app.receipt.survivors.clear();
        app.receipt.not_checked = vec!["fix: setup `mvn package` exited 1".into()];
        app.go(Screen::Receipt);
        let s = render(&app, 140, 40);
        for want in [
            "NOTHING CHECKED",
            "nothing was proven",
            "no check ran",
            "not checked",
            "setup `mvn package` exited 1",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
        for gone in [
            "what was proven",
            "look here first",
            "0 of 0 checks",
            "NOT STARTED",
        ] {
            assert!(!s.contains(gone), "still shows {gone:?}\n{s}");
        }
    }

    #[test]
    fn a_run_that_never_reached_a_check_is_one_faint_line_under_the_list() {
        let mut halted = conductor_model::demo::receipt();
        halted.run_id = "HALT1".into();
        halted.checks.clear();
        let app = App::from_receipts(vec![halted, conductor_model::demo::receipt()]);
        let s = render(&app, 140, 40);
        assert!(
            s.contains("+ 1 run that never reached a check · conductor receipt HALT1"),
            "{s}"
        );
    }

    struct Reviewed(LiveRun, conductor_model::view::Review);
    impl crate::app::RunSource for Reviewed {
        fn receipts(&self) -> Vec<conductor_model::Receipt> {
            vec![]
        }
        fn running(&self) -> Vec<LiveRun> {
            vec![self.0.clone()]
        }
        fn live(&self, _id: &str) -> Option<LiveRun> {
            Some(self.0.clone())
        }
        fn review(&self, _id: &str) -> Option<conductor_model::view::Review> {
            Some(self.1.clone())
        }
    }

    #[test]
    fn a_run_waiting_on_a_decision_shows_the_five_things_and_the_decision() {
        use conductor_model::view::*;
        let mut l = conductor_model::demo::live();
        l.run_id = "R9".into();
        l.waiting = Some(Waiting {
            stage: "review".into(),
            who: "PO".into(),
            question: "Approve only if every criterion is shown.".into(),
            since: "2026-09-26T00:00:00Z".into(),
            ask: None,
        });
        let review = Review {
            run_id: "R9".into(),
            stage: "review".into(),
            who: "PO".into(),
            title: "BUG-101: Balance request fails".into(),
            criteria: vec![
                "AC1: `GET /accounts/ACC-1/balance` returns 200".into(),
                "AC2: ACC-2 returns 0.00".into(),
            ],
            change: Change {
                files: vec!["src/main/java/LedgerService.java".into()],
                added: 2,
                removed: 2,
                branch: "conductor/R9".into(),
            },
            evidence: vec![EvidenceRow {
                stage: "fix".into(),
                check: "command_assert".into(),
                claim: "`./scripts/deploy-and-test.sh` succeeds".into(),
                before: Some(Verdict::Failed),
                after: Verdict::Passed,
                detail: "exited 0".into(),
                lines: vec![
                    "PASS AC1: GET /accounts/ACC-1/balance -> HTTP 200".into(),
                    "PASS AC2: ...".into(),
                ],
            }],
            cost: Cost {
                tries: 1,
                tokens: 394573,
                cost_usd: 1.14,
                waited_s: 26,
                wall_s: 126,
            },
        };
        let mut app = App::from_source(Box::new(Reviewed(l, review)));
        app.go(Screen::Live);
        app.tick();
        let s = render(&app, 140, 45);
        for want in [
            "decide: BUG-101: Balance request fails",
            "the ticket asks",
            "AC2: ACC-2 returns 0.00",
            "what changed",
            "+2 −2 in 1 file",
            "src/main/java/LedgerService.java",
            "what the checks say",
            "failed before → passed after",
            "PASS AC1",
            "1 try · 394573 tokens · $1.14 · waited on people 26s · 2m06s so far",
            "waiting for PO",
            "approve",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
        // The working view's columns are not on this screen.
        assert!(!s.contains("conductor opens a pane per stage"), "{s}");
    }

    #[test]
    fn the_live_screen_shows_checks_evidence_and_panes() {
        let mut app = App::demo();
        app.go(Screen::Live);
        let s = render(&app, 140, 40);
        for want in [
            "implement",
            "run by conductor",
            "witnessed",
            "inferred",
            "herdr tab 2",
            "pane open",
            "receipt so far",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
    }

    #[test]
    fn the_live_screen_offers_the_receipt_when_done() {
        let mut app = App::demo();
        app.go(Screen::Live);
        for _ in 0..20 {
            app.tick();
        }
        let s = render(&app, 140, 40);
        assert!(s.contains("The receipt is ready"), "{s}");
    }

    #[test]
    fn a_failed_headless_run_says_so_and_offers_no_herdr_tab() {
        let mut live = conductor_model::demo::live();
        live.herdr_tab = None;
        live.panes.clear();
        live.ended = Some(Verdict::Failed);
        let mut app = App::from_runs(vec![], vec![live]);
        app.go(Screen::Live);
        let s = render(&app, 140, 40);
        assert!(s.contains("the run failed"), "{s}");
        assert!(s.contains("headless"), "{s}");
        assert!(!s.contains("watch in tab"), "{s}");
        assert!(!s.contains("herdr tab"), "{s}");
    }

    #[test]
    fn the_receipt_screen_leads_with_the_verdict_and_what_was_proven() {
        let mut app = App::demo();
        app.go(Screen::Receipt);
        let s = render(&app, 140, 40);
        for want in [
            "PASSED",
            "5 of 5 checks",
            "Tests were written by a different agent",
            "look here first",
            "not checked",
            "Also at",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
        assert!(
            !s.contains("anchored in"),
            "how-it-ran is folded away until asked for"
        );
        app.show_how = true;
        assert!(render(&app, 140, 40).contains("anchored in 4e1c0d2"));
    }

    #[test]
    fn the_launch_screen_asks_three_questions() {
        let mut app = App::demo();
        app.go(Screen::Launch);
        let s = render(&app, 140, 40);
        for want in [
            "What kind of work?",
            "What should it build?",
            "Where should it run?",
            "tickets/PTT-1240.md",
            "Start run",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
    }

    #[test]
    fn every_screen_draws_in_a_small_terminal_without_panicking() {
        let mut app = App::demo();
        for s in Screen::ALL {
            app.go(s);
            let _ = render(&app, 60, 16);
        }
    }
}
