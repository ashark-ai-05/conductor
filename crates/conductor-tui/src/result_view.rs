//! Curated native result layouts. Data chooses a supported view, never executable UI.
use crate::{
    theme::Theme,
    workspace::{clean, line, text_block},
};
use conductor_model::task::{Presentation, TestResults};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, Table, TableState, Wrap},
};

pub struct View {
    pub title: String,
    pub summary: String,
    pub kind: &'static str,
    pub columns: Vec<String>,
    pub rows: Vec<Entry>,
}
pub struct Entry {
    pub cells: Vec<String>,
    pub sources: Vec<usize>,
    pub evidence: Option<usize>,
    pub failed: bool,
}
impl View {
    pub fn answer(p: Presentation) -> Self {
        match p {
            Presentation::Facts {
                title,
                summary,
                facts,
            } => Self {
                title,
                summary,
                kind: "Key facts",
                columns: vec!["Fact".into(), "Value".into()],
                rows: facts
                    .into_iter()
                    .map(|f| Entry {
                        cells: vec![f.label, f.value],
                        sources: f.sources,
                        evidence: None,
                        failed: false,
                    })
                    .collect(),
            },
            Presentation::Table {
                title,
                summary,
                columns,
                rows,
            } => Self {
                title,
                summary,
                kind: "Table",
                columns,
                rows: rows
                    .into_iter()
                    .map(|r| Entry {
                        cells: r.cells,
                        sources: r.sources,
                        evidence: None,
                        failed: false,
                    })
                    .collect(),
            },
        }
    }
    pub fn tests(results: &TestResults) -> Self {
        let count = |name: &str| results.cases.iter().filter(|c| c.outcome == name).count();
        Self {
            title: if results.cases.is_empty() {
                "No test cases recorded".into()
            } else {
                format!(
                    "{} failed · {} passed · {} ignored",
                    count("Failed"),
                    count("Passed"),
                    count("Ignored")
                )
            },
            summary: format!(
                "Recorded test cases · latest completed attempt per stage\n{}",
                results.notices.join("\n")
            ),
            kind: "Test report",
            columns: vec!["Result".into(), "Test".into(), "Stage / attempt".into()],
            rows: results
                .cases
                .iter()
                .map(|c| Entry {
                    cells: vec![
                        c.outcome.clone(),
                        c.name.clone(),
                        format!("{} / {}", c.stage, c.attempt),
                    ],
                    sources: vec![],
                    evidence: Some(c.evidence),
                    failed: c.outcome == "Failed",
                })
                .collect(),
        }
    }
    pub fn detail(&self, index: usize) -> String {
        self.rows
            .get(index)
            .map(|r| {
                self.columns
                    .iter()
                    .zip(&r.cells)
                    .map(|(label, value)| format!("{label}: {value}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }
}

pub fn draw(f: &mut Frame, area: Rect, view: &View, selected: usize, t: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if view.kind == "Key facts" {
        draw_facts(f, area, view, selected, t);
        return;
    }
    let summary = Paragraph::new(clean(&view.summary)).wrap(Wrap { trim: false });
    // The summary remains available in full in the text view. Reserve space for rows.
    let summary_height = summary
        .line_count(area.width)
        .min(6)
        .min(area.height.saturating_sub(6) as usize) as u16;
    let [heading, subtitle, body] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(summary_height + 1),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(vec![
            line(view.kind, t.fg(t.accent)),
            line(&view.title, t.bold()),
        ]),
        heading,
    );
    let [summary_area, overflow] =
        Layout::vertical([Constraint::Length(summary_height), Constraint::Length(1)])
            .areas(subtitle);
    if summary.line_count(area.width) > summary_height as usize {
        f.render_widget(
            Paragraph::new("… v full text for all details").style(t.fg(t.accent)),
            overflow,
        );
    }
    f.render_widget(summary.style(t.dim()), summary_area);
    if view.rows.is_empty() {
        text_block(
            f,
            body,
            vec![line(
                "No test cases recorded. v full output · e evidence",
                t.dim(),
            )],
            0,
        );
        return;
    }
    let mut widths: Vec<u16> = view
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let widest = view
                .rows
                .iter()
                .map(|r| Line::raw(clean(&r.cells[i])).width())
                .max()
                .unwrap_or(0);
            (widest.max(c.len()).min(36) as u16).max(8)
        })
        .collect();
    let refs = view.kind != "Test report";
    let source_width = 10;
    let needed = widths.iter().sum::<u16>()
        + (widths.len() as u16 * 2)
        + if refs { source_width + 2 } else { 0 }
        + 2;
    let selected = selected.min(view.rows.len() - 1);
    if needed > body.width {
        // A narrow pane uses labelled, wrapped fields; no hidden comparison columns.
        let row = &view.rows[selected];
        let mut lines = vec![
            line(
                format!(
                    "{} of {} · ↑↓ select · Enter evidence",
                    selected + 1,
                    view.rows.len()
                ),
                t.fg(t.accent),
            ),
            Line::raw(""),
        ];
        for (label, value) in view.columns.iter().zip(&row.cells) {
            lines.push(line(label, t.dim()));
            lines.push(line(
                value,
                if row.failed { t.fg(t.fail) } else { t.bold() },
            ));
            lines.push(Line::raw(""));
        }
        if refs {
            lines.push(line(references(&row.sources), t.fg(t.accent)));
        }
        text_block(f, body, lines, 0);
        return;
    }
    // Keep related values and their sources close together on wide terminals.
    let mut headings: Vec<_> = view.columns.iter().map(|c| Cell::from(clean(c))).collect();
    if refs {
        widths.push(source_width);
        headings.push(Cell::from("Sources"));
    }
    let rows = view.rows.iter().map(|entry| {
        let mut values = entry.cells.clone();
        if refs {
            values.push(references(&entry.sources));
        }
        let mut height = 1;
        let cells: Vec<_> = values
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(index, (value, width))| {
                let text = wrap(value, *width);
                height = height.max(text.len() as u16);
                Cell::from(text).style(if refs && index == values.len() - 1 {
                    t.fg(t.accent)
                } else if view.kind == "Key facts" && index == 1 {
                    t.bold()
                } else {
                    t.text()
                })
            })
            .collect();
        Row::new(cells)
            .height(height)
            .bottom_margin(1)
            .style(if entry.failed { t.fg(t.fail) } else { t.text() })
    });
    let table = Table::new(rows, widths.iter().copied().map(Constraint::Length))
        .header(Row::new(headings).style(t.dim()).bottom_margin(1))
        .column_spacing(2)
        .row_highlight_style(t.text().bg(t.sel))
        .highlight_symbol("▎ ");
    let mut state = TableState::default().with_selected(selected);
    f.render_stateful_widget(
        table,
        Rect {
            width: needed.min(body.width),
            ..body
        },
        &mut state,
    );
}

fn references(refs: &[usize]) -> String {
    if refs.is_empty() {
        "—".into()
    } else {
        refs.iter()
            .map(|i| format!("[{i}]"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// Ratatui cells do not wrap automatically. Split on terminal display width, not bytes.
fn wrap(text: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![];
    let mut current = String::new();
    for c in clean(text).chars() {
        let mut candidate = current.clone();
        candidate.push(c);
        if Line::raw(candidate).width() > width as usize && !current.is_empty() {
            lines.push(Line::raw(std::mem::take(&mut current)));
        }
        current.push(c);
    }
    lines.push(Line::raw(current));
    lines
}

/// Preferred content height, so a short answer does not push follow-up to the pane bottom.
pub fn height(view: &View, width: u16) -> u16 {
    if view.kind == "Key facts" {
        return facts_height(view, width);
    }
    let summary = Paragraph::new(clean(&view.summary))
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .min(6) as u16;
    if view.kind != "Key facts" {
        let needed = view
            .columns
            .iter()
            .enumerate()
            .map(|(i, label)| {
                view.rows
                    .iter()
                    .filter_map(|r| r.cells.get(i))
                    .map(|s| Line::raw(clean(s)).width())
                    .max()
                    .unwrap_or(0)
                    .max(label.len())
                    .clamp(8, 36)
            })
            .sum::<usize>()
            + view.columns.len() * 2
            + 14;
        if needed > width as usize {
            let fields = view
                .columns
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    view.rows
                        .iter()
                        .filter_map(|r| r.cells.get(i))
                        .map(|s| {
                            Paragraph::new(clean(s))
                                .wrap(Wrap { trim: false })
                                .line_count(width.max(1))
                        })
                        .max()
                        .unwrap_or(1)
                        + 2
                })
                .sum::<usize>();
            return summary
                .saturating_add(8)
                .saturating_add(fields.min(u16::MAX as usize) as u16);
        }
    }
    let rows = view
        .rows
        .iter()
        .map(|r| {
            let width = width.saturating_sub(25).max(1);
            r.cells
                .iter()
                .map(|s| {
                    Paragraph::new(clean(s))
                        .wrap(Wrap { trim: false })
                        .line_count(width)
                })
                .max()
                .unwrap_or(1)
                .saturating_add(1)
        })
        .sum::<usize>()
        .min(u16::MAX as usize) as u16;
    summary.saturating_add(rows).saturating_add(5)
}

struct FactsLayout {
    width: u16,
    columns: usize,
    cell_width: u16,
    heading: Vec<Line<'static>>,
    cells: Vec<Vec<Line<'static>>>,
}

fn facts_layout(view: &View, width: u16, selected: usize, t: &Theme) -> FactsLayout {
    use crate::document;
    use ratatui::text::Span;
    let width = width.min(document::MAX_WIDTH);
    let inner = width.saturating_sub(4).max(1);
    let columns = if inner >= 72 && view.rows.len() <= 12 {
        2
    } else {
        1
    };
    let cell_width = if columns == 2 {
        inner.saturating_sub(3) / 2
    } else {
        inner
    };
    let mut heading = document::wrap(
        &[Span::styled(clean(&view.title), t.bold())],
        inner,
        "",
        "",
        true,
    );
    heading.push(Line::raw(""));
    let summary = document::wrap(
        &[Span::styled(clean(&view.summary), t.dim())],
        inner,
        "",
        "",
        true,
    );
    heading.extend(summary.iter().take(4).cloned());
    if summary.len() > 4 {
        heading.push(line("… v full text for all context", t.fg(t.accent)));
    }
    heading.push(Line::raw(""));
    let label_width = view
        .rows
        .iter()
        .filter_map(|r| r.cells.first())
        .map(|s| Line::raw(clean(s)).width())
        .max()
        .unwrap_or(0)
        .min(22);
    let cells = view
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let label = row.cells.first().map(String::as_str).unwrap_or("");
            let value = row.cells.get(1).map(String::as_str).unwrap_or("");
            let marker = if i == selected { "▎ " } else { "  " };
            let refs = if row.sources.is_empty() {
                String::new()
            } else {
                format!("  {}", references(&row.sources))
            };
            let value_style = if i == selected {
                t.bold().fg(t.accent)
            } else {
                t.bold()
            };
            if columns == 1 && cell_width >= 54 {
                let padding = label_width.saturating_sub(Line::raw(clean(label)).width());
                document::wrap(
                    &[
                        Span::styled(
                            format!("{}{label}{}  ", marker, " ".repeat(padding)),
                            t.dim(),
                        ),
                        Span::styled(clean(value), value_style),
                        Span::styled(refs, t.fg(t.accent)),
                    ],
                    cell_width,
                    "",
                    "  ",
                    true,
                )
            } else {
                let mut lines = document::wrap(
                    &[Span::styled(format!("{label}{refs}"), t.dim())],
                    cell_width,
                    marker,
                    "  ",
                    true,
                );
                lines.extend(document::wrap(
                    &[Span::styled(clean(value), value_style)],
                    cell_width,
                    "  ",
                    "  ",
                    true,
                ));
                lines
            }
        })
        .collect();
    FactsLayout {
        width,
        columns,
        cell_width,
        heading,
        cells,
    }
}

fn facts_height(view: &View, width: u16) -> u16 {
    let layout = facts_layout(view, width, 0, &Theme::DARK);
    let cells = layout
        .cells
        .chunks(layout.columns)
        .map(|band| band.iter().map(Vec::len).max().unwrap_or(0) + 1)
        .sum::<usize>();
    (layout.heading.len() + cells + 2).min(u16::MAX as usize) as u16
}

fn draw_facts(f: &mut Frame, area: Rect, view: &View, selected: usize, t: &Theme) {
    use ratatui::widgets::{Block, Borders, Padding};
    let selected = selected.min(view.rows.len().saturating_sub(1));
    let layout = facts_layout(view, area.width, selected, t);
    let area = Rect {
        width: layout.width,
        ..area
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(t.fg(t.line))
        .title(" Facts ")
        .title_style(t.fg(t.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let heading_height =
        (layout.heading.len().min(u16::MAX as usize) as u16).min(inner.height.saturating_sub(2));
    let [heading, body] =
        Layout::vertical([Constraint::Length(heading_height), Constraint::Min(0)]).areas(inner);
    f.render_widget(Paragraph::new(layout.heading), heading);
    if body.height == 0 || body.width == 0 {
        return;
    }
    let bands: Vec<_> = layout.cells.chunks(layout.columns).collect();
    let heights: Vec<_> = bands
        .iter()
        .map(|b| b.iter().map(Vec::len).max().unwrap_or(0) + 1)
        .collect();
    let selected_band = selected / layout.columns;
    let mut start = selected_band.min(bands.len().saturating_sub(1));
    let mut used = heights.get(start).copied().unwrap_or(0);
    while start > 0 && used + heights[start - 1] <= body.height as usize {
        start -= 1;
        used += heights[start];
    }
    let mut y = body.y;
    for (band_index, band) in bands.iter().enumerate().skip(start) {
        let height = heights[band_index].saturating_sub(1).min(u16::MAX as usize) as u16;
        if y >= body.bottom() {
            break;
        }
        for (column, lines) in band.iter().enumerate() {
            let cell = Rect::new(
                body.x + column as u16 * (layout.cell_width + 3),
                y,
                layout.cell_width.min(body.width),
                height.min(body.bottom().saturating_sub(y)),
            );
            let style = if band_index * layout.columns + column == selected {
                t.text().bg(t.sel)
            } else {
                t.text()
            };
            f.render_widget(Paragraph::new(lines.clone()).style(style), cell);
        }
        y = y.saturating_add(height + 1);
    }
}

pub fn draw_checks(f: &mut Frame, area: Rect, view: &View, selected: usize, t: &Theme) {
    use ratatui::widgets::{List, ListItem, ListState};
    // Put the verdict before check metadata so narrow panes cannot hide it.
    let gate = view
        .summary
        .lines()
        .skip(1)
        .map(|notice| {
            let compact = notice.splitn(3, " · ").nth(2).unwrap_or(notice);
            if let Some((check, verdict)) = compact.rsplit_once(" · gate ") {
                format!("Gate {verdict} · {check}")
            } else {
                compact.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let notice = Paragraph::new(clean(&format!("{}\n{}", view.title, gate)))
        .style(t.dim())
        .wrap(Wrap { trim: false });
    let full_height = notice.line_count(area.width.max(1));
    let notice_height = full_height
        .min(5)
        .min(area.height.saturating_sub(3) as usize) as u16;
    let overflow = u16::from(full_height > notice_height as usize);
    let [heading, notices, more, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(notice_height),
        Constraint::Length(overflow),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new("Checks · selected saved attempt")
            .style(t.bold())
            .wrap(Wrap { trim: false }),
        heading,
    );
    f.render_widget(notice, notices);
    if overflow > 0 {
        f.render_widget(
            Paragraph::new("… v full record for all check details").style(t.fg(t.accent)),
            more,
        );
    }
    if view.rows.is_empty() {
        text_block(
            f,
            body,
            vec![line(
                "No cases recorded. e Evidence for the check result.",
                t.fg(t.warn),
            )],
            0,
        );
        return;
    }
    let rows = view
        .rows
        .iter()
        .map(|row| {
            let result = row.cells.first().map(String::as_str).unwrap_or("Unknown");
            let name = row.cells.get(1).map(String::as_str).unwrap_or("");
            let (glyph, color) = match result {
                "Passed" => ("✓", t.pass),
                "Failed" => ("×", t.fail),
                _ => ("○", t.warn),
            };
            let mut lines = wrap(
                &format!("{glyph} {name}"),
                area.width.saturating_sub(2).max(1),
            );
            for l in &mut lines {
                *l = l.clone().style(t.fg(color));
            }
            lines.push(Line::raw(""));
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();
    let mut state =
        ListState::default().with_selected(Some(selected.min(rows.len().saturating_sub(1))));
    f.render_stateful_widget(
        List::new(rows)
            .highlight_symbol("▎ ")
            .highlight_style(t.text().bg(t.sel)),
        body,
        &mut state,
    );
}
