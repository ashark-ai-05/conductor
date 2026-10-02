//! CommonMark to terminal cells. Parsing, layout and scrolling never invoke an agent.
use crate::{theme::Theme, workspace::clean};
use conductor_model::task::Citation;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The text width inside a pane of `width`: the pane's full width, less border and padding.
pub fn inner_width(width: u16) -> u16 {
    width.saturating_sub(4).max(1)
}

/// Wrap styled graphemes with hanging indentation; code keeps its whitespace.
pub fn wrap(
    spans: &[Span<'static>],
    width: u16,
    first: &str,
    rest: &str,
    prose: bool,
) -> Vec<Line<'static>> {
    let units: Vec<_> = spans
        .iter()
        .flat_map(|s| {
            s.content
                .graphemes(true)
                .map(move |g| (g.to_owned(), s.style))
        })
        .collect();
    let mut result = vec![];
    let mut start = 0;
    loop {
        let prefix = if result.is_empty() { first } else { rest };
        let prefix: String = prefix
            .graphemes(true)
            .scan(0, |used, g| {
                *used += g.width();
                (*used < width as usize).then_some(g)
            })
            .collect();
        let available = (width as usize).saturating_sub(prefix.width()).max(1);
        let mut end = start;
        let mut used = 0;
        let mut space = None;
        while end < units.len() {
            let size = units[end].0.width();
            if used + size > available && end > start {
                break;
            }
            used += size;
            if units[end].0.chars().all(char::is_whitespace) {
                space = Some(end);
            }
            end += 1;
        }
        let mut next = end;
        if prose
            && end < units.len()
            && let Some(at) = space.filter(|at| *at > start)
        {
            end = at;
            next = at + 1;
        }
        if prose {
            while next < units.len() && units[next].0.chars().all(char::is_whitespace) {
                next += 1;
            }
        }
        let mut row = vec![Span::raw(prefix)];
        for (g, style) in &units[start..end] {
            if let Some(last) = row.last_mut()
                && last.style == *style
            {
                last.content.to_mut().push_str(g);
            } else {
                row.push(Span::styled(g.clone(), *style));
            }
        }
        result.push(Line::from(row));
        if next >= units.len() {
            break;
        }
        start = next;
    }
    result
}

pub fn plain(text: &str, width: u16, t: &Theme) -> Vec<Line<'static>> {
    text.lines()
        .flat_map(|s| {
            wrap(
                &[Span::styled(clean(s).replace('\t', "    "), t.text())],
                width,
                "",
                "",
                false,
            )
        })
        .collect()
}

#[derive(Default)]
struct TableData {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    row: Vec<Vec<Span<'static>>>,
}
struct Renderer<'a> {
    t: &'a Theme,
    width: u16,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    prefix: String,
    quote: usize,
    /// The language and text of the fenced block being read.
    code: Option<(String, String)>,
    table: Option<TableData>,
    citations: &'a [Citation],
    links: Vec<String>,
    markers: Vec<String>,
}
impl Renderer<'_> {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or(self.t.text())
    }
    fn emit(&mut self, text: impl Into<String>) {
        self.current
            .push(Span::styled(clean(&text.into()), self.style()));
    }
    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }
    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let first = format!("{}{}", "│ ".repeat(self.quote), self.prefix);
        let rest = format!(
            "{}{}",
            "│ ".repeat(self.quote),
            " ".repeat(self.prefix.width())
        );
        let marked = !self.prefix.trim().is_empty() || self.quote > 0;
        let mut rows = wrap(
            &std::mem::take(&mut self.current),
            self.width,
            &first,
            &rest,
            true,
        );
        // List markers and quote bars carry the accent, so structure shows at a glance.
        if marked {
            for row in &mut rows {
                if let Some(lead) = row.spans.first_mut() {
                    lead.style = self.t.fg(self.t.accent);
                }
            }
        }
        self.lines.extend(rows);
        self.prefix = " ".repeat(self.prefix.width());
    }
    fn gap(&mut self) {
        self.flush();
        if self.lines.last().is_some_and(|l| l.width() > 0) {
            self.lines.push(Line::raw(""));
        }
    }
    /// A fenced block, drawn by what it holds: a formula, a diagram, or code. Each sits on
    /// its own surface with a coloured bar, and keeps its text exactly as written.
    fn block(&mut self, language: &str, text: &str) {
        let t = self.t;
        let indent = " ".repeat(self.prefix.width());
        let room = (self.width as usize)
            .saturating_sub(indent.len() + 3)
            .max(1) as u16;
        let diagram = (language == "mermaid")
            .then(|| crate::diagram::mermaid(text, room, t))
            .flatten();
        let (bar, label) = match language {
            "math" => (t.run, "Formula".to_owned()),
            "mermaid" if diagram.is_some() => (t.accent, "Diagram".to_owned()),
            "mermaid" => (t.warn, "Diagram · shown as written".to_owned()),
            "" => (t.pass, "Code".to_owned()),
            other => (t.pass, other.to_owned()),
        };
        let body = if language == "math" {
            t.bold().bg(t.surface)
        } else {
            t.text().bg(t.surface)
        };
        let rows: Vec<Line<'static>> = match diagram {
            Some(rows) => rows,
            None => text
                .trim_end_matches('\n')
                .split('\n')
                .flat_map(|row| {
                    wrap(
                        &[Span::styled(clean(row).replace('\t', "    "), body)],
                        room,
                        "",
                        "",
                        false,
                    )
                })
                .collect(),
        };
        let widest = rows.iter().map(Line::width).max().unwrap_or(0);
        self.lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(label, t.fg(bar)),
        ]));
        for row in rows {
            let pad = widest.saturating_sub(row.width()) + 1;
            let mut spans = vec![
                Span::raw(indent.clone()),
                Span::styled("▌ ", t.fg(bar).bg(t.surface)),
            ];
            spans.extend(row.spans.into_iter().map(|mut span| {
                span.style = span.style.bg(t.surface);
                span
            }));
            spans.push(Span::styled(" ".repeat(pad), Style::new().bg(t.surface)));
            self.lines.push(Line::from(spans));
        }
    }
    fn link(&mut self, url: &str) {
        let marker = if let Some(index) = self.citations.iter().position(|c| c.url == url) {
            format!(" [{}]", index + 1)
        } else {
            let index = self.links.iter().position(|u| u == url).unwrap_or_else(|| {
                self.links.push(clean(url));
                self.links.len() - 1
            });
            format!(" [link {}]", index + 1)
        };
        self.markers.push(marker);
        self.push_style(self.t.fg(self.t.accent).add_modifier(Modifier::UNDERLINED));
    }
    fn table(&mut self, table: TableData) {
        let Some(headers) = table.rows.first() else {
            return;
        };
        let count = headers.len();
        if count == 0 {
            return;
        }
        let mut widths: Vec<usize> = (0..count)
            .map(|i| {
                table
                    .rows
                    .iter()
                    .filter_map(|r| r.get(i))
                    .map(|s| s.iter().map(Span::width).sum::<usize>())
                    .max()
                    .unwrap_or(1)
                    .clamp(4, 36)
            })
            .collect();
        let room = (self.width as usize).saturating_sub(count.saturating_sub(1) * 3);
        if room < count * 8 {
            for (n, row) in table.rows.iter().skip(1).enumerate() {
                self.lines
                    .push(Line::styled(format!("Row {}", n + 1), self.t.dim()));
                for (i, cell) in row.iter().enumerate() {
                    let label = headers
                        .get(i)
                        .map(|s| s.iter().map(|s| s.content.as_ref()).collect::<String>())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("Column {}", i + 1));
                    self.lines.extend(wrap(
                        &[Span::styled(label, self.t.dim())],
                        self.width,
                        "",
                        "",
                        true,
                    ));
                    self.lines.extend(wrap(cell, self.width, "", "", true));
                }
                self.lines.push(Line::raw(""));
            }
            return;
        }
        while widths.iter().sum::<usize>() > room {
            if let Some((i, _)) = widths.iter().enumerate().max_by_key(|(_, v)| *v) {
                widths[i] -= 1;
            }
        }
        for (n, row) in table.rows.iter().enumerate() {
            if n == 0 && row.iter().all(|c| c.iter().all(|s| s.content.is_empty())) {
                continue;
            }
            let cells: Vec<_> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| wrap(cell, *width as u16, "", "", true))
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1);
            for y in 0..height {
                let mut spans = vec![];
                for (i, cell) in cells.iter().enumerate() {
                    let line = cell.get(y).cloned().unwrap_or_default();
                    let size = line.width();
                    spans.extend(line.spans.into_iter().map(|mut s| {
                        if n == 0 {
                            s.style = s.style.patch(self.t.bold());
                        }
                        s
                    }));
                    if i + 1 < count {
                        spans.push(Span::raw(" ".repeat(widths[i].saturating_sub(size))));
                        spans.push(Span::styled(" │ ", self.t.faint()));
                    }
                }
                self.lines.push(Line::from(spans));
            }
            if n == 0 {
                self.lines.push(Line::styled(
                    "─".repeat(
                        (widths.iter().sum::<usize>() + (count - 1) * 3).min(self.width as usize),
                    ),
                    self.t.faint(),
                ));
            }
        }
        self.gap();
    }
}

pub fn lines(text: &str, width: u16, t: &Theme, citations: &[Citation]) -> Vec<Line<'static>> {
    let mut r = Renderer {
        t,
        width: width.max(1),
        lines: vec![],
        current: vec![],
        styles: vec![],
        lists: vec![],
        prefix: String::new(),
        quote: 0,
        code: None,
        table: None,
        citations,
        links: vec![],
        markers: vec![],
    };
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    // Maths is rewritten first: the Markdown parser would eat its backslashes.
    let text = crate::content::math(text);
    for event in Parser::new_ext(&text, options) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                r.flush();
                if r.lists.is_empty() {
                    r.gap();
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                r.gap();
                r.push_style(t.bold().fg(t.accent));
                // Top headings carry a bar, so sections stand out when scrolling.
                if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                    r.current.push(Span::styled("▍", t.fg(t.run)));
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                r.flush();
                r.styles.pop();
                r.gap();
            }
            Event::Start(Tag::Strong) => r.push_style(Style::new().add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Emphasis) => {
                r.push_style(Style::new().add_modifier(Modifier::ITALIC))
            }
            Event::Start(Tag::Strikethrough) => {
                r.push_style(Style::new().add_modifier(Modifier::CROSSED_OUT))
            }
            Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough) => {
                r.styles.pop();
            }
            Event::Start(Tag::List(start)) => {
                r.flush();
                r.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                r.flush();
                r.lists.pop();
                r.prefix = "  ".repeat(r.lists.len());
                if r.lists.is_empty() {
                    r.gap();
                }
            }
            Event::Start(Tag::Item) => {
                r.flush();
                let marker = match r.lists.last_mut() {
                    Some(Some(n)) => {
                        let label = format!("{n}. ");
                        *n += 1;
                        label
                    }
                    _ => "• ".into(),
                };
                r.prefix = format!("{}{marker}", "  ".repeat(r.lists.len().saturating_sub(1)));
            }
            Event::End(TagEnd::Item) => r.flush(),
            Event::Start(Tag::BlockQuote(_)) => {
                r.gap();
                r.quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                r.flush();
                r.quote = r.quote.saturating_sub(1);
                r.gap();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                r.gap();
                let language = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    _ => String::new(),
                };
                r.code = Some((language, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, text)) = r.code.take() {
                    r.block(&language, &text);
                }
                r.gap();
            }
            Event::Text(text) if r.code.is_some() => {
                if let Some((_, body)) = &mut r.code {
                    body.push_str(&text);
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                r.emit(text.to_string())
            }
            Event::Code(code) => r
                .current
                .push(Span::styled(clean(&code), r.style().fg(t.accent).bg(t.sel))),
            Event::SoftBreak => r.emit(" "),
            Event::HardBreak => r.flush(),
            Event::Rule => {
                r.gap();
                r.lines
                    .push(Line::styled("─".repeat(r.width as usize), t.faint()));
                r.gap();
            }
            Event::TaskListMarker(checked) => r.emit(if checked { "☑ " } else { "☐ " }),
            Event::Start(Tag::Link { dest_url, .. }) => r.link(&dest_url),
            Event::Start(Tag::Image { dest_url, .. }) => {
                r.emit("Image: ");
                r.link(&dest_url);
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                r.styles.pop();
                if let Some(marker) = r.markers.pop() {
                    r.current.push(Span::styled(marker, t.fg(t.accent)));
                }
            }
            Event::FootnoteReference(name) => r.emit(format!("[{name}]")),
            Event::Start(Tag::FootnoteDefinition(name)) => {
                r.gap();
                r.emit(format!("[{name}] "));
            }
            Event::End(TagEnd::FootnoteDefinition) => r.gap(),
            Event::Start(Tag::Table(_)) => {
                r.gap();
                r.table = Some(TableData::default());
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = &mut r.table {
                    table.row.push(std::mem::take(&mut r.current));
                }
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                if let Some(table) = &mut r.table {
                    table.rows.push(std::mem::take(&mut table.row));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(table) = r.table.take() {
                    r.table(table);
                }
            }
            _ => {}
        }
    }
    r.flush();
    if !r.links.is_empty() {
        r.gap();
        r.lines.push(Line::styled("Links", t.bold()));
        for (i, url) in r.links.iter().enumerate() {
            r.lines.extend(wrap(
                &[Span::styled(format!("[link {}] {url}", i + 1), t.dim())],
                r.width,
                "",
                "",
                true,
            ));
        }
    }
    while r.lines.last().is_some_and(|l| l.width() == 0) {
        r.lines.pop();
    }
    r.lines
}

/// Draws a scrolling document in a pane and returns how far it can scroll.
pub fn draw(
    f: &mut Frame,
    area: Rect,
    text: Vec<Line<'static>>,
    scroll: u16,
    title: &str,
    focused: bool,
    t: &Theme,
) -> u16 {
    let mut block = crate::workspace::pane(t, title, focused);
    let inner = block.inner(area);
    let max = text
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = scroll.min(max);
    if max > 0 {
        block = block.title_bottom(format!(
            " {}–{} of {} ",
            scroll as usize + 1,
            (scroll as usize + inner.height as usize).min(text.len()),
            text.len()
        ));
    }
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(text).scroll((scroll, 0)), inner);
    crate::workspace::scrollbar(
        f,
        area.inner(ratatui::layout::Margin {
            horizontal: 0,
            vertical: 1,
        }),
        scroll,
        max,
    );
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn styles_lists_links_and_code_are_rendered_without_changing_literals() {
        let md = "# Heading\n\n**Strong** and *emphasis* with `**literal**`.\n\n- A long list item that wraps and keeps its indentation\n  - Nested\n\n```rust\nlet stars = \"**keep**\";\n    indented();\n```\n\n[Apply](https://example.com/jobs)";
        let lines = lines(md, 32, &Theme::DARK, &[]);
        let out = text(&lines);
        assert!(!out.contains("# Heading"));
        assert!(!out.contains("**Strong**"));
        assert!(out.contains("**literal**"));
        assert!(out.contains("**keep**"));
        assert!(!out.contains("```"));
        assert!(out.contains("• Nested"));
        assert!(out.contains("Apply [link 1]"));
        assert!(out.contains("https://example.com/jobs"));
        assert!(
            lines
                .iter()
                .flat_map(|l| &l.spans)
                .any(|s| s.content.contains("Strong")
                    && s.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(lines.iter().all(|l| l.width() <= 32));
    }
    #[test]
    fn tables_reflow_and_keep_every_cell_at_split_pane_widths() {
        let md = "| City | Temperature |\n|---|---|\n| Melbourne | 14°C |\n| 東京 | 22°C |";
        for width in [12, 24, 60, 94] {
            let lines = lines(md, width, &Theme::DARK, &[]);
            let out = text(&lines);
            assert!(out.contains("Melbourne"));
            assert!(out.contains("14°C"));
            assert!(out.contains("東京"));
            assert!(!out.contains("|---|"));
            assert!(lines.iter().all(|l| l.width() <= width as usize));
        }
    }
    #[test]
    fn known_citations_keep_their_numbers_without_a_second_url_list() {
        let citations = vec![Citation {
            url: "https://example.com/jobs".into(),
            ..Citation::default()
        }];
        let out = text(&lines(
            "[Apply](https://example.com/jobs)",
            60,
            &Theme::DARK,
            &citations,
        ));
        assert_eq!(out, "Apply [1]");
    }
}
