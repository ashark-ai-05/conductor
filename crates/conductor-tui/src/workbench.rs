//! Workbench chrome and read-only records. All content is projected from saved data.
use crate::{
    document,
    theme::Theme,
    workspace::{Workspace, clean, line, text_block},
};
use conductor_model::agent::{AgentAvailability, AgentKind, AgentSelection};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub struct AgentPicker {
    pub selection: AgentSelection,
    pub model: String,
    pub editing_model: bool,
    pub choices: Vec<AgentAvailability>,
    pub error: Option<String>,
}

impl AgentPicker {
    pub fn new(selection: AgentSelection, choices: Vec<AgentAvailability>) -> Self {
        Self {
            model: if selection.kind == AgentKind::Amp {
                selection.mode.clone()
            } else {
                selection.model.clone()
            }
            .unwrap_or_default(),
            selection,
            editing_model: false,
            choices,
            error: None,
        }
    }
    pub fn on_key(&mut self, key: KeyEvent) -> Option<AgentSelection> {
        match key.code {
            KeyCode::Tab => self.editing_model = !self.editing_model,
            KeyCode::Up | KeyCode::Down if !self.editing_model => {
                let choices = AgentKind::ALL;
                let current = choices
                    .iter()
                    .position(|k| *k == self.selection.kind)
                    .unwrap_or(0);
                let step = if key.code == KeyCode::Up {
                    choices.len() - 1
                } else {
                    1
                };
                self.selection.kind = choices[(current + step) % choices.len()];
                self.selection.model = None;
                self.selection.mode = None;
                self.model.clear();
                self.error = None;
            }
            KeyCode::Char(c)
                if self.editing_model
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && self.model.len() < 200 =>
            {
                self.model.push(c);
                self.error = None;
            }
            KeyCode::Backspace if self.editing_model => {
                self.model.pop();
                self.error = None;
            }
            KeyCode::Enter => {
                let value = (!self.model.trim().is_empty()).then(|| self.model.trim().into());
                if self.selection.kind == AgentKind::Amp {
                    self.selection.mode = value;
                    self.selection.model = None;
                } else {
                    self.selection.model = value;
                    self.selection.mode = None;
                }
                let error = self
                    .selection
                    .validate()
                    .err()
                    .map(str::to_owned)
                    .or_else(|| {
                        self.choices
                            .iter()
                            .find(|a| a.kind == self.selection.kind)
                            .and_then(|a| a.unavailable.clone())
                    });
                if let Some(error) = error {
                    self.error = Some(error);
                } else {
                    return Some(self.selection.clone());
                }
            }
            _ => {}
        }
        None
    }
}

pub fn draw_picker(f: &mut Frame, area: Rect, picker: &AgentPicker, t: &Theme) {
    let mut lines = vec![
        line("Choose an agent for a new question", t.bold()),
        line(
            "Existing tasks keep their original agent and model.",
            t.dim(),
        ),
        Line::raw(""),
    ];
    let mut focus_line = 0;
    for kind in AgentKind::ALL {
        let selected = kind == picker.selection.kind;
        if selected {
            focus_line = lines.len();
        }
        lines.push(line(
            format!("{} {}", if selected { "●" } else { "○" }, kind.label()),
            if selected {
                t.bold().fg(t.accent)
            } else {
                t.text()
            },
        ));
        lines.push(line(format!("  {}", kind.scope()), t.dim()));
        if let Some(why) = picker
            .choices
            .iter()
            .find(|a| a.kind == kind)
            .and_then(|a| a.unavailable.as_ref())
        {
            lines.push(line(why, t.fg(t.warn)));
        }
        lines.push(Line::raw(""));
    }
    if picker.editing_model {
        focus_line = lines.len();
    }
    lines.push(line(
        if picker.selection.kind == AgentKind::Amp {
            "Mode (optional)"
        } else {
            "Model ID (optional)"
        },
        t.bold(),
    ));
    lines.push(line(
        format!(
            "{}{}",
            if picker.model.is_empty() && !picker.editing_model {
                "Runtime default"
            } else {
                &picker.model
            },
            if picker.editing_model { "▏" } else { "" }
        ),
        t.fg(t.accent),
    ));
    lines.push(line(
        if picker.selection.kind == AgentKind::Amp {
            "Use a mode supported by amp --help; Amp chooses its models."
        } else {
            "Use a model your runtime supports; Pi also accepts provider/model."
        },
        t.dim(),
    ));
    lines.push(line(
        "Credentials stay with the runtime. Availability is checked when starting.",
        t.dim(),
    ));
    lines.push(Line::raw(""));
    lines.push(line(
        "↑↓ agent   Tab model/mode   Enter use selection   Esc cancel",
        t.text(),
    ));
    if let Some(error) = &picker.error {
        focus_line = lines.len();
        lines.push(line(error, t.fg(t.fail)));
    }
    let block = Block::default()
        .title(" Agent & model ")
        .title_style(t.bold())
        .borders(Borders::ALL)
        .border_style(t.fg(t.line));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let before = Paragraph::new(lines[..focus_line].to_vec())
        .wrap(Wrap { trim: false })
        .line_count(inner.width.max(1));
    let scroll = before
        .saturating_sub(inner.height.saturating_sub(6) as usize)
        .min(u16::MAX as usize) as u16;
    text_block(f, inner, lines, scroll);
}

pub fn agent_label(app: &Workspace) -> String {
    match &app.detail {
        Some(d) if d.native.is_some() => "Native · no agent".into(),
        Some(d) if d.question.is_some() => d.question.as_ref().unwrap().agent.label(),
        Some(_) => "Workflow runtime".into(),
        None => app.agent.label(),
    }
}

pub fn record(app: &Workspace) -> (String, String) {
    let Some(d) = &app.detail else {
        return ("Record".into(), "No task selected.".into());
    };
    if let Some(q) = &d.question {
        let mut text = format!(
            "## Answer record\nTask: {}\nAnswer: {} of {}\nAgent requested: {}\n\n",
            q.id,
            app.turn + 1,
            q.turns.len(),
            q.agent.label()
        );
        if let Some(turn) = app.current_turn() {
            text.push_str(&format!(
                "State: {}\nStarted: {}\nTokens: {}\nCost: {}\n\n",
                turn.state.label(),
                turn.started_at,
                turn.tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "Unavailable".into()),
                turn.cost_usd
                    .map(|n| format!("${n:.4}"))
                    .unwrap_or_else(|| "Unavailable".into())
            ));
            if let Some(answer) = &turn.answer {
                let captured = answer
                    .sources
                    .iter()
                    .filter(|s| s.captured.is_some())
                    .count();
                text.push_str(&format!(
                    "## Sources\n{} citations · {} matching captures\n",
                    answer.sources.len(),
                    captured
                ));
                for (i, source) in answer.sources.iter().enumerate() {
                    text.push_str(&format!(
                        "- [{}] {} · {}\n",
                        i + 1,
                        source.title,
                        if source.captured.is_some() {
                            "tool output captured"
                        } else {
                            "citation only"
                        }
                    ));
                }
            } else {
                text.push_str("No answer recorded for this request.\n");
            }
            if let Some(error) = &turn.error {
                text.push_str(&format!("\n## Stopping point\n{error}\n"));
            }
        }
        text.push_str("\n## Not independently checked\nCitations and conclusions are agent supplied. A captured fetch does not establish factual accuracy.\nThis is local question history, not a hash-chained workflow receipt.");
        return ("Source record".into(), text);
    }
    if let Some(native) = &d.native {
        return (
            "Capture record".into(),
            format!(
                "## {}\n{}\n\n{}\n\nCaptured local data; imported claims are not acceptance checks.",
                d.summary.title,
                native.media_type,
                d.evidence
                    .iter()
                    .map(|(title, body)| format!("{title}\n{body}"))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            ),
        );
    }
    let Some(receipt) = &d.receipt else {
        return (
            "Receipt".into(),
            format!(
                "## No completed receipt\nTask: {}\nState: {}\n\nSaved changes and individual checks can be inspected in Evidence. No completed run verdict is implied.",
                d.summary.id,
                d.summary.state.label()
            ),
        );
    };
    let mut text = format!(
        "## {}\nRun: {}\nRecorded verdict: {}\n{} of {} checks passed\n\n## Checks\n",
        receipt.work,
        receipt.run_id,
        receipt.verdict().word(),
        receipt.passed_count(),
        receipt.checks.len()
    );
    for check in &receipt.checks {
        text.push_str(&format!(
            "- {} {}\n  {} · {}\n",
            check.verdict.glyph(),
            check.claim,
            check.source,
            check.detail
        ));
    }
    text.push_str("\n## Not checked\n");
    if receipt.not_checked.is_empty() {
        text.push_str(
            "No additional gaps were recorded. Coverage is limited to the configured checks.\n",
        );
    }
    for gap in &receipt.not_checked {
        text.push_str(&format!("- {gap}\n"));
    }
    text.push_str("\n## Recorded execution\n");
    for (key, value) in &receipt.how {
        text.push_str(&format!("{key}: {value}\n"));
    }
    text.push_str(&format!("\n## Record integrity\nChain: {}\nAnchor: {}\n\nThis view reads the saved receipt. Run conductor verify {} to re-check record integrity. Integrity does not establish factual correctness.", receipt.integrity.chain_head, receipt.integrity.anchored_in.as_deref().unwrap_or("Not anchored"), receipt.run_id));
    ("Receipt".into(), text)
}

pub fn draw_record(f: &mut Frame, area: Rect, app: &Workspace, t: &Theme) {
    let Some((title, text)) = &app.record_snapshot else {
        return;
    };
    let block = Block::default()
        .title(format!(" {} · saved snapshot ", clean(title)))
        .borders(Borders::TOP | Borders::LEFT)
        .border_style(t.fg(t.line));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines = vec![line("Esc back · ↑↓ scroll", t.dim()), Line::raw("")];
    lines.extend(document::lines(text, inner.width, t, &[]));
    text_block(f, inner, lines, app.panel_scroll);
}
