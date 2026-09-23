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
        Paragraph::new(Line::from(vec![
            Span::styled("herdr ", t.faint()),
            Span::styled("✓", t.fg(t.pass)),
            Span::styled(" · traces → Tempo ", t.faint()),
        ]))
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
            let ks = if *hot {
                t.fg(t.accent).add_modifier(Modifier::BOLD)
            } else {
                t.text().add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(format!(" {k} "), ks.bg(t.sel)));
            spans.push(Span::styled(
                format!(" {what}   "),
                if *hot { t.text() } else { t.dim() },
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
    let [intro, _, stats, _, banner, list, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(if needs.is_some() { 3 } else { 0 }),
        Constraint::Length(app.runs.len() as u16 + 6),
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
}

// ── live ────────────────────────────────────────────────────────────────────

fn live(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let run = &app.live;
    let done = app.live_done();
    let [head, _, pipe, _, ready, cols, _, panes, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(if done { 3 } else { 0 }),
        Constraint::Length(10),
        Constraint::Length(1),
        Constraint::Length(5),
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
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" t ", t.bold().bg(t.sel)),
            Span::styled(
                format!(" watch in tab {}   ", run.herdr_tab.unwrap_or(2)),
                t.dim(),
            ),
            Span::styled(" r ", t.fg(t.accent).add_modifier(Modifier::BOLD).bg(t.sel)),
            Span::styled(" receipt so far", t.text()),
        ]))
        .right_aligned(),
        h2,
    );

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
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " ✓ every check passed ",
                    t.fg(t.pass).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" The receipt is ready. ", t.text()),
                Span::styled("⏎ or r to open it", t.fg(t.accent)),
            ]))
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(t.fg(t.pass))
                    .style(Style::new().bg(t.pass_bg)),
            ),
            ready,
        );
    }

    let [checks, log] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(2)
        .areas(cols);
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
    let title = format!("herdr tab {}", run.herdr_tab.unwrap_or(2));
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

// ── receipt ─────────────────────────────────────────────────────────────────

fn receipt(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let r = &app.receipt;
    let v = r.verdict();
    let how_height = if app.show_how {
        r.how.len() as u16 + 5
    } else {
        1
    };
    let [banner, _, proven, _, pair, _, how, reach] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(r.checks.len() as u16 + 3),
        Constraint::Length(1),
        Constraint::Length(r.survivors.len().max(r.not_checked.len()).min(4) as u16 + 3),
        Constraint::Length(1),
        Constraint::Length(how_height),
        Constraint::Length(1),
    ])
    .areas(area);

    let bg = if v == Verdict::Passed {
        t.pass_bg
    } else {
        t.blocked_bg
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} ", v.word().to_uppercase()),
                t.verdict(v).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {} · {}   ", r.work, r.run_id), t.text()),
            Span::styled(
                format!("{} of {} checks", r.passed_count(), r.checks.len()),
                t.fg(t.pass),
            ),
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
    f.render_widget(
        Paragraph::new(rows).block(block(
            t,
            "what was proven",
            "each of these could have failed",
            false,
        )),
        proven,
    );

    let [look, gaps] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(2)
        .areas(pair);
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
    f.render_widget(
        Paragraph::new(s_lines)
            .wrap(Wrap { trim: false })
            .block(block(t, "look here first", "bugs the tests missed", true)),
        look,
    );
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
    f.render_widget(
        Paragraph::new(n_lines)
            .wrap(Wrap { trim: false })
            .block(block(t, "not checked", "", false)),
        gaps,
    );

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
