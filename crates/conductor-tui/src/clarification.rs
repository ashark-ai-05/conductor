//! Native single-choice input, with a free-text route for every request.
use crate::{document, theme::Theme, workspace::clean};
use conductor_model::interaction::{InputBinding, InputRequest, InputResponse};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph},
};

#[derive(Clone)]
pub struct Draft {
    pub binding: InputBinding,
    pub highlighted: usize,
    pub selected: Option<usize>,
    pub text: String,
    pub context: String,
}
impl Draft {
    pub fn new(binding: InputBinding) -> Self {
        Self {
            binding,
            highlighted: 0,
            selected: None,
            text: String::new(),
            context: String::new(),
        }
    }
}
fn wrap(s: &str, width: u16, t: &Theme, strong: bool) -> Vec<Line<'static>> {
    document::wrap(
        &[Span::styled(
            clean(s),
            if strong { t.bold() } else { t.dim() },
        )],
        width.max(1),
        "",
        "",
        true,
    )
}
pub fn height(request: &InputRequest, width: u16, t: &Theme) -> u16 {
    let width = document::inner_width(width);
    let heading = wrap(&request.question, width, t, true).len()
        + wrap(&request.explanation, width, t, false).len().min(3)
        + 2;
    let rows: usize = request
        .options
        .iter()
        .map(|c| {
            wrap(&c.label, width.saturating_sub(4), t, true).len()
                + wrap(&c.description, width.saturating_sub(4), t, false).len()
                + 1
        })
        .sum();
    (heading + rows + 7).min(u16::MAX as usize) as u16
}
pub fn draw(
    f: &mut Frame,
    area: Rect,
    request: &InputRequest,
    draft: &Draft,
    active: bool,
    response: Option<&InputResponse>,
    t: &Theme,
) {
    let area = Rect {
        width: area.width.min(document::MAX_WIDTH),
        ..area
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(t.fg(t.line))
        .title(if active {
            " Clarify "
        } else {
            " Clarification · history "
        })
        .title_style(t.fg(t.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let mut heading = wrap(&request.question, inner.width, t, true);
    if !request.explanation.is_empty() {
        heading.push(Line::raw(""));
        let description = wrap(&request.explanation, inner.width, t, false);
        heading.extend(description.iter().take(3).cloned());
        if description.len() > 3 {
            heading.push(Line::styled("v original for full context", t.dim()));
        }
    }
    heading.push(Line::raw(""));
    let [head, body, foot] = Layout::vertical([
        Constraint::Length((heading.len() as u16).min(inner.height.saturating_sub(5))),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(heading), head);
    let selected = if active {
        draft.selected
    } else {
        response
            .and_then(|r| r.option.as_ref())
            .and_then(|id| request.options.iter().position(|c| &c.id == id))
            .or_else(|| response.map(|_| request.options.len()))
    };
    let options: Vec<_> = request
        .options
        .iter()
        .map(|c| (c.label.as_str(), c.description.as_str()))
        .chain(std::iter::once((
            "My own answer",
            "Use different wording or add a missing interpretation.",
        )))
        .collect();
    let full_height: usize = options
        .iter()
        .map(|(label, detail)| {
            wrap(&format!("○ {label}"), body.width.saturating_sub(2), t, true).len()
                + document::wrap(
                    &[Span::styled(clean(detail), t.dim())],
                    body.width.saturating_sub(2).max(1),
                    "  ",
                    "  ",
                    true,
                )
                .len()
                + 1
        })
        .sum::<usize>()
        .saturating_sub(1);
    let compact = (body.height as usize) < full_height;
    let items: Vec<_> = options
        .iter()
        .copied()
        .enumerate()
        .map(|(i, (label, detail))| {
            let mut lines = wrap(
                &format!("{} {label}", if selected == Some(i) { "●" } else { "○" }),
                body.width.saturating_sub(2),
                t,
                true,
            );
            if !compact {
                lines.extend(document::wrap(
                    &[Span::styled(clean(detail), t.dim())],
                    body.width.saturating_sub(2).max(1),
                    "  ",
                    "  ",
                    true,
                ));
                if i < request.options.len() {
                    lines.push(Line::raw(""));
                }
            }
            ListItem::new(lines)
        })
        .collect();
    let mut state =
        ListState::default().with_selected(Some(draft.highlighted.min(request.options.len())));
    f.render_stateful_widget(
        List::new(items)
            .highlight_symbol("▎ ")
            .highlight_style(t.text().bg(t.sel)),
        body,
        &mut state,
    );
    let note = if active {
        if !draft.context.is_empty() {
            format!("Context: {}", draft.context)
        } else if selected == Some(request.options.len()) && !draft.text.is_empty() {
            format!("Your answer: {}", draft.text)
        } else if compact {
            format!(
                "{} of {} · {}",
                draft.highlighted + 1,
                options.len(),
                options
                    .get(draft.highlighted)
                    .map(|(_, detail)| *detail)
                    .unwrap_or("")
            )
        } else {
            "Choose one, or write your own answer.".into()
        }
    } else if let Some(r) = response {
        let value = r
            .option
            .as_ref()
            .and_then(|id| request.options.iter().find(|c| &c.id == id))
            .map(|c| c.label.as_str())
            .unwrap_or(&r.text);
        format!(
            "Recorded: {value}{}",
            if r.context.is_empty() {
                String::new()
            } else {
                format!(" · {}", r.context)
            }
        )
    } else {
        "No longer waiting here. ] opens the next turn.".into()
    };
    let mut lines = wrap(&note, foot.width, t, false);
    if lines.len() > 2 {
        lines.truncate(2);
        lines.push(Line::styled(
            if active {
                "… c edit context · f edit answer"
            } else {
                "… Enter response details"
            },
            t.dim(),
        ));
    }
    f.render_widget(Paragraph::new(lines), foot);
}
