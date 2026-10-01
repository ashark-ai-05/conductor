//! One task stays selected through work, evidence inspection, and a decision.
use crate::{
    app::RunSource,
    clarification, document,
    result_view::{self, View},
    theme::Theme,
    workbench,
};
use conductor_model::capability::{Available, Binding};
use conductor_model::interaction::{InputBinding, InputRequest, InputResponse};
use conductor_model::task::{
    Activity, Answer, Citation, Fact, Presentation, Question, ResultRow, State, TaskDetail,
    TaskSummary, TestResult, TestResults, Turn,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::sync::{Arc, mpsc};

/// Host integration runs outside the terminal drawing loop.
pub trait TaskHost {
    fn publish(&mut self, id: &str, title: &str, state: State);
}

pub trait TaskSource: RunSource + Send + Sync {
    fn host(&self) -> Option<Box<dyn TaskHost>> {
        None
    }
    fn open_agent(&self, _id: &str) -> Result<(), String> {
        Err("This task has no agent pane.".into())
    }
    fn has_agent(&self, _id: &str) -> bool {
        false
    }
    fn capabilities(&self) -> Result<Vec<Available>, String> {
        Ok(vec![])
    }
    fn invoke(&self, _binding: &Binding, _parent: Option<&str>) -> Result<String, String> {
        Err("Native operations are unavailable here.".into())
    }
    fn agents(&self) -> Vec<conductor_model::agent::AgentAvailability> {
        vec![]
    }
    fn question_with_agent(
        &self,
        parent: Option<&str>,
        input: &str,
        agent: &conductor_model::agent::AgentSelection,
    ) -> Result<String, String> {
        if agent != &Default::default() {
            return Err("This source does not support agent selection.".into());
        }
        self.question(parent, input)
    }
    fn tasks(&self) -> Vec<TaskSummary>;
    fn task(&self, id: &str) -> Option<TaskDetail>;
    fn question(&self, parent: Option<&str>, input: &str) -> Result<String, String>;
    fn respond(&self, _response: &InputResponse) -> Result<String, String> {
        Err("Structured replies are unavailable here.".into())
    }
    fn cancel_question(&self, id: &str) -> Result<(), String>;
    fn open_source(&self, _url: &str) -> Result<(), String> {
        Err("Opening sources is unavailable here.".into())
    }
    fn copy(&self, _text: &str) -> Result<(), String> {
        Err("Clipboard is unavailable here.".into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Record,
    Capabilities,
    Sources,
    Activity,
    Evidence,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Question,
    Workflow,
    FollowUp,
    ClarificationAnswer,
    ClarificationContext,
    Decision(bool),
}

pub struct Workspace {
    pub agent: conductor_model::agent::AgentSelection,
    pub agent_picker: Option<workbench::AgentPicker>,
    pub record_snapshot: Option<(String, String)>,
    follow_up_drafts: std::collections::HashMap<String, String>,
    source: Option<Arc<dyn TaskSource>>,
    pub tasks: Vec<TaskSummary>,
    pub capabilities: Vec<Available>,
    pub selected: usize,
    pub detail: Option<TaskDetail>,
    pub input: Option<Input>,
    pub text: String,
    pub cursor: usize,
    pub panel: Option<Panel>,
    pub list_focus: bool,
    pub focused: bool,
    pub agent_available: bool,
    host: Option<Box<dyn TaskHost>>,
    pub scroll: u16,
    pub panel_scroll: u16,
    pub item: usize,
    pub expanded: bool,
    pub turn: usize,
    pub result_row: usize,
    pub text_view: bool,
    pub change: usize,
    pub change_focus: bool,
    pub evidence_snapshot: Option<(String, String)>,
    pub source_filter: Option<Vec<usize>>,
    pub selection_detail: Option<String>,
    pub workflows: Vec<String>,
    pub workflow: usize,
    pub workflow_focus: bool,
    pub status: Option<String>,
    pub quit: bool,
    pub demo: bool,
    clarification: Option<clarification::Draft>,
    pending: Option<mpsc::Receiver<Result<String, String>>>,
}

impl Workspace {
    pub fn new(source: Arc<dyn TaskSource>) -> Self {
        let tasks = source.tasks();
        let workflows = source.workflows();
        let host = source.host();
        Self {
            agent: Default::default(),
            agent_picker: None,
            record_snapshot: None,
            follow_up_drafts: Default::default(),
            capabilities: vec![],
            host,
            source: Some(source),
            tasks,
            workflows,
            selected: 0,
            detail: None,
            input: Some(Input::Question),
            text: String::new(),
            cursor: 0,
            panel: None,
            list_focus: false,
            focused: false,
            agent_available: false,
            scroll: 0,
            panel_scroll: 0,
            item: 0,
            expanded: false,
            turn: 0,
            result_row: 0,
            text_view: false,
            change: 0,
            change_focus: false,
            evidence_snapshot: None,
            source_filter: None,
            selection_detail: None,
            workflow: 0,
            workflow_focus: false,
            status: None,
            quit: false,
            demo: false,
            clarification: None,
            pending: None,
        }
    }

    pub fn demo() -> Self {
        let detail = demo_task();
        Self {
            agent: Default::default(),
            agent_picker: None,
            record_snapshot: None,
            follow_up_drafts: Default::default(),
            capabilities: vec![],
            source: None,
            host: None,
            tasks: vec![
                detail.summary.clone(),
                demo_comparison(false).summary,
                demo_comparison(true).summary,
                demo_series().summary,
                demo_tests().summary,
                demo_bug().summary,
                demo_clarification().summary,
            ],
            workflows: vec![],
            selected: 0,
            detail: Some(detail),
            input: None,
            text: String::new(),
            cursor: 0,
            panel: None,
            list_focus: false,
            focused: false,
            agent_available: false,
            scroll: 0,
            panel_scroll: 0,
            item: 0,
            expanded: false,
            turn: 0,
            result_row: 0,
            text_view: false,
            change: 0,
            change_focus: false,
            evidence_snapshot: None,
            source_filter: None,
            selection_detail: None,
            workflow: 0,
            workflow_focus: false,
            status: None,
            quit: false,
            demo: true,
            clarification: None,
            pending: None,
        }
    }

    pub fn open(&mut self, id: &str) -> bool {
        self.keep_follow_up();
        let detail = self
            .source
            .as_ref()
            .and_then(|s| s.task(id))
            .or_else(|| {
                if self.demo {
                    match id {
                        "q-demo" => Some(demo_task()),
                        "q-clarification" => Some(demo_clarification()),
                        "q-comparison" => Some(demo_comparison(false)),
                        "q-forecast" => Some(demo_comparison(true)),
                        "q-series" => Some(demo_series()),
                        "test-demo" => Some(demo_tests()),
                        "bug-demo" => Some(demo_bug()),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .or_else(|| self.detail.as_ref().filter(|d| d.summary.id == id).cloned());
        let Some(detail) = detail else {
            self.status = Some("This task could not be read.".into());
            return false;
        };
        self.selected = self.tasks.iter().position(|t| t.id == id).unwrap_or(0);
        self.turn = detail
            .question
            .as_ref()
            .map(|q| q.turns.len().saturating_sub(1))
            .unwrap_or(0);
        self.detail = Some(detail);
        self.input = None;
        self.list_focus = false;
        self.panel = None;
        self.scroll = 0;
        self.panel_scroll = 0;
        self.item = 0;
        self.expanded = false;
        self.result_row = 0;
        self.text_view = false;
        self.change = self
            .detail
            .as_ref()
            .and_then(|d| {
                d.changes
                    .last()
                    .and_then(|last| d.changes.iter().position(|c| c.stage == last.stage))
            })
            .unwrap_or(0);
        self.change_focus = false;
        self.evidence_snapshot = None;
        self.source_filter = None;
        self.selection_detail = None;
        self.status = None;
        self.record_snapshot = None;
        self.sync_clarification();
        true
    }

    pub fn tick(&mut self) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(Ok(id)) => {
                    if self.input == Some(Input::FollowUp) {
                        self.follow_up_drafts.remove(&id);
                        self.input = None;
                    }
                    self.pending = None;
                    self.open(&id);
                }
                Ok(Err(e)) => {
                    self.pending = None;
                    self.status = Some(e);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.status = Some(
                        "The launch process ended unexpectedly. Your input is still here.".into(),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(src) = &self.source {
            let keep = if self.list_focus {
                self.tasks.get(self.selected).map(|t| t.id.clone())
            } else {
                self.detail.as_ref().map(|d| d.summary.id.clone())
            };
            self.tasks = src.tasks();
            self.selected = keep
                .and_then(|id| self.tasks.iter().position(|t| t.id == id))
                .unwrap_or(0);
            if let Some(d) = &self.detail {
                let latest = src.task(&d.summary.id);
                if let Some(latest) = &latest {
                    // Keep an expanded activity entry anchored while new events arrive.
                    if self.panel == Some(Panel::Activity) && self.expanded {
                        let new_len = latest
                            .question
                            .as_ref()
                            .and_then(|q| q.turns.get(self.turn))
                            .map(|t| t.activity.len())
                            .unwrap_or(latest.activity.len());
                        self.item += new_len.saturating_sub(self.activity().len());
                    }
                }
                if let Some(next) = &latest
                    && let Some(selected) = d.tests.as_ref().and_then(|r| {
                        r.cases
                            .iter()
                            .filter(|case| {
                                d.changes.get(self.change).is_none_or(|c| {
                                    c.stage == case.stage && c.attempt == case.attempt
                                })
                            })
                            .nth(self.result_row)
                    })
                    && let Some(cases) = next.tests.as_ref().map(|r| &r.cases)
                    && let Some(index) = cases
                        .iter()
                        .filter(|case| {
                            d.changes
                                .get(self.change)
                                .is_none_or(|c| c.stage == case.stage && c.attempt == case.attempt)
                        })
                        .position(|c| c.id == selected.id)
                {
                    self.result_row = index;
                    if self.panel == Some(Panel::Evidence) && self.selection_detail.is_some() {
                        self.item = cases.iter().find(|c| c.id == selected.id).unwrap().evidence;
                    }
                }
                if let Some(next) = &latest {
                    if let Some(selected) = d.changes.get(self.change) {
                        self.change = next
                            .changes
                            .iter()
                            .position(|c| c.stage == selected.stage && c.path == selected.path)
                            .unwrap_or(0);
                    } else {
                        self.change = 0;
                    }
                    if d.question
                        .as_ref()
                        .and_then(Question::pending_input)
                        .is_none()
                        && next
                            .question
                            .as_ref()
                            .and_then(Question::pending_input)
                            .is_some()
                        && self.panel == Some(Panel::Activity)
                        && !self.expanded
                        && self.input.is_none()
                    {
                        self.panel = None;
                    }
                    self.detail = latest;
                }
            }
        }
        self.sync_clarification();
        if let Some(view) = self.result_view() {
            self.result_row = self.result_row.min(view.rows.len().saturating_sub(1));
        }
        if let Some(d) = &self.detail {
            self.agent_available = self
                .source
                .as_ref()
                .is_some_and(|s| s.has_agent(&d.summary.id));
            if let Some(host) = &mut self.host {
                host.publish(&d.summary.id, &d.summary.title, d.summary.state);
            }
        } else if let Some(host) = &mut self.host {
            host.publish("", "Conductor", State::Answered);
        }
    }

    pub fn clarification_request(&self) -> Option<(InputBinding, InputRequest)> {
        self.detail.as_ref()?.question.as_ref()?.input_at(self.turn)
    }
    fn clarification_pending(&self) -> bool {
        let Some((binding, _)) = self.clarification_request() else {
            return false;
        };
        self.detail
            .as_ref()
            .and_then(|d| d.question.as_ref())
            .and_then(Question::pending_input)
            .is_some_and(|(pending, _)| pending == binding)
    }
    fn sync_clarification(&mut self) {
        if matches!(
            self.input,
            Some(Input::ClarificationAnswer | Input::ClarificationContext)
        ) {
            return;
        }
        if let Some((binding, _)) = self.clarification_request() {
            if self
                .clarification
                .as_ref()
                .is_none_or(|d| d.binding != binding)
            {
                self.clarification = Some(clarification::Draft::new(binding));
            }
        } else {
            self.clarification = None;
        }
    }
    fn edit_clarification(&mut self, context: bool) {
        if !self.clarification_pending() {
            return;
        }
        self.sync_clarification();
        let Some(draft) = &mut self.clarification else {
            return;
        };
        if !context {
            let count = self
                .detail
                .as_ref()
                .and_then(|d| d.question.as_ref())
                .and_then(|q| q.input_at(self.turn))
                .map(|(_, r)| r.options.len())
                .unwrap_or(0);
            draft.selected = Some(count);
            draft.highlighted = count;
        }
        self.text = if context {
            draft.context.clone()
        } else {
            draft.text.clone()
        };
        self.cursor = self.text.len();
        self.input = Some(if context {
            Input::ClarificationContext
        } else {
            Input::ClarificationAnswer
        });
        self.panel = None;
        self.text_view = false;
        self.status = None;
    }
    fn keep_clarification_text(&mut self) {
        if let Some(draft) = &mut self.clarification {
            match self.input {
                Some(Input::ClarificationAnswer) => draft.text = self.text.clone(),
                Some(Input::ClarificationContext) => draft.context = self.text.clone(),
                _ => {}
            }
        }
    }
    fn submit_clarification(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let Some((binding, request)) = self.clarification_request() else {
            return;
        };
        let Some(draft) = &self.clarification else {
            return;
        };
        if !self.clarification_pending() || draft.binding != binding {
            self.status=Some("This clarification is no longer current. Esc returns to the task; ] opens the next turn.".into());
            return;
        }
        let Some(selected) = draft.selected else {
            self.status = Some("Space selects an option; f lets you write your own answer.".into());
            return;
        };
        if selected == request.options.len() && draft.text.trim().is_empty() {
            self.edit_clarification(false);
            return;
        }
        let response = InputResponse {
            binding: draft.binding.clone(),
            option: request.options.get(selected).map(|c| c.id.clone()),
            text: if selected == request.options.len() {
                draft.text.clone()
            } else {
                String::new()
            },
            context: draft.context.clone(),
        };
        if let Err(why) = self
            .detail
            .as_ref()
            .unwrap()
            .question
            .as_ref()
            .unwrap()
            .resolve_input(&response)
        {
            self.status = Some(why.into());
            return;
        }
        let Some(src) = self.source.clone() else {
            self.status = Some("Demo only. No response was sent.".into());
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(src.respond(&response));
        });
        self.pending = Some(rx);
        self.status = Some("Submitting your response…".into());
    }

    pub fn current_turn(&self) -> Option<&Turn> {
        self.detail
            .as_ref()?
            .question
            .as_ref()?
            .turns
            .get(self.turn)
    }

    /// A retry may retain the answer to the same question, with its original sources.
    /// A different follow-up must never borrow an answer to another question.
    pub fn answer_turn(&self) -> Option<&Turn> {
        let current = self.current_turn()?;
        if current.answer.is_some() {
            return Some(current);
        }
        self.detail.as_ref()?.question.as_ref()?.turns[..self.turn]
            .iter()
            .rev()
            .find(|turn| turn.question == current.question && turn.answer.is_some())
    }

    pub fn current_answer(&self) -> Option<&Answer> {
        self.answer_turn()
            .and_then(|t| t.answer.as_ref())
            .or_else(|| self.detail.as_ref()?.native.as_ref()?.answer.as_ref())
    }

    pub fn answer_is_previous(&self) -> bool {
        self.current_turn().is_some_and(|q| q.answer.is_none()) && self.current_answer().is_some()
    }

    fn result_view(&self) -> Option<View> {
        if let Some(answer) = self.current_answer() {
            answer.presentation().ok().flatten().map(|p| {
                let mut view = View::answer(p);
                if self.detail.as_ref().is_some_and(|d| d.native.is_some()) {
                    for row in &mut view.rows {
                        row.evidence = Some(0);
                    }
                }
                view
            })
        } else if self.current_turn().is_none() {
            let d = self.detail.as_ref()?;
            let report = d.tests.as_ref()?;
            if let Some(change) = d.changes.get(self.change) {
                let matched = TestResults {
                    cases: report
                        .cases
                        .iter()
                        .filter(|c| c.stage == change.stage && c.attempt == change.attempt)
                        .cloned()
                        .collect(),
                    notices: report
                        .notices
                        .iter()
                        .filter(|n| {
                            n.starts_with(&format!(
                                "{} · attempt {} ·",
                                change.stage, change.attempt
                            ))
                        })
                        .cloned()
                        .collect(),
                };
                Some(View::tests(&matched))
            } else {
                Some(View::tests(report))
            }
        } else {
            None
        }
    }

    fn source_index(&self) -> Option<usize> {
        match &self.source_filter {
            Some(refs) => refs.get(self.item).copied(),
            None => Some(self.item),
        }
    }

    fn inspect_result(&mut self) {
        let Some(view) = self.result_view() else {
            return;
        };
        let Some(row) = view.rows.get(self.result_row) else {
            return;
        };
        self.selection_detail = Some(view.detail(self.result_row));
        self.panel_scroll = 0;
        if let Some(evidence) = row.evidence {
            self.panel = Some(Panel::Evidence);
            self.item = evidence;
            self.evidence_snapshot = self
                .detail
                .as_ref()
                .and_then(|d| d.evidence.get(evidence))
                .cloned();
            self.expanded = true;
        } else {
            self.panel = Some(Panel::Sources);
            self.source_filter = Some(
                row.sources
                    .iter()
                    .map(|n| n - 1)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            );
            self.item = 0;
            self.expanded = self.items() == 1;
        }
    }

    fn items(&self) -> usize {
        match self.panel {
            Some(Panel::Record) => 0,
            Some(Panel::Capabilities) => self.capabilities.len(),
            Some(Panel::Sources) => self
                .current_answer()
                .map(|a| {
                    self.source_filter
                        .as_ref()
                        .map_or(a.sources.len(), Vec::len)
                })
                .unwrap_or(0),
            Some(Panel::Activity) => self.activity().len(),
            Some(Panel::Evidence) => self.detail.as_ref().map(|d| d.evidence.len()).unwrap_or(0),
            None => 0,
        }
    }

    pub fn activity(&self) -> &[Activity] {
        self.current_turn()
            .map(|t| t.activity.as_slice())
            .or_else(|| self.detail.as_ref().map(|d| d.activity.as_slice()))
            .unwrap_or(&[])
    }

    fn keep_follow_up(&mut self) {
        if self.input == Some(Input::FollowUp)
            && let Some(d) = &self.detail
        {
            self.follow_up_drafts
                .insert(d.summary.id.clone(), self.text.clone());
        }
    }

    fn compose(&mut self, input: Input) {
        self.keep_follow_up();
        self.input = Some(input);
        self.text = if input == Input::FollowUp {
            self.detail
                .as_ref()
                .and_then(|d| self.follow_up_drafts.get(&d.summary.id))
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.cursor = self.text.len();
        self.panel = None;
        if input != Input::FollowUp {
            self.scroll = 0;
        }
        self.list_focus = false;
        self.status = None;
        self.workflow_focus = false;
        if matches!(input, Input::Question | Input::Workflow) {
            self.detail = None;
        }
    }

    fn load_capabilities(&mut self) {
        self.capabilities.clear();
        if let Some(src) = &self.source {
            match src.capabilities() {
                Ok(items) => {
                    self.capabilities = items;
                    self.status = None;
                }
                Err(error) => self.status = Some(error),
            }
        }
        self.item = 0;
    }

    fn invoke(&mut self, binding: Binding, parent: Option<String>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(src.invoke(&binding, parent.as_deref()));
        });
        self.pending = Some(rx);
        self.panel = None;
        self.input = None;
        self.status = Some("Reading source · no agent…".into());
    }

    pub fn toggle_panel(&mut self, panel: Panel) {
        self.evidence_snapshot = None;
        let filtered = self.source_filter.take().is_some();
        self.selection_detail = None;
        self.panel = if self.panel == Some(panel) && !filtered {
            None
        } else {
            Some(panel)
        };
        if self.panel == Some(Panel::Capabilities) {
            self.load_capabilities();
        }
        if self.panel == Some(Panel::Record) {
            self.record_snapshot = Some(workbench::record(self));
        }
        self.item = 0;
        self.expanded = self.panel == Some(Panel::Record)
            || self.panel == Some(Panel::Sources) && self.items() == 1;
        self.panel_scroll = 0;
    }

    fn submit(&mut self) {
        if self.pending.is_some() {
            return;
        }
        if matches!(
            self.input,
            Some(Input::ClarificationAnswer | Input::ClarificationContext)
        ) {
            self.keep_clarification_text();
            if self.input == Some(Input::ClarificationContext) {
                self.input = None;
                self.status = None;
            } else {
                self.submit_clarification();
            }
            return;
        }
        let text = self.text.trim().to_owned();
        if text.is_empty() {
            self.status = Some("Type your request first.".into());
            return;
        }
        let Some(src) = self.source.clone() else {
            self.status =
                Some("Demo only. Open conductor ui without --demo to ask a question.".into());
            return;
        };
        let Some(input) = self.input else { return };
        if let Input::Decision(yes) = input {
            let Some(d) = &self.detail else { return };
            let Some(waiting) = &d.waiting else {
                self.status = Some("This task is no longer waiting for a decision.".into());
                return;
            };
            match src.decide(&d.summary.id, &waiting.stage, yes, &text) {
                Ok(()) => {
                    self.input = None;
                    self.status = Some("Decision recorded.".into());
                }
                Err(e) => self.status = Some(e),
            }
            return;
        }
        let workflow = self.workflows.get(self.workflow).cloned();
        if input == Input::Workflow {
            let Some(w) = &workflow else {
                self.status = Some(
                    "No workflows in this repository. Questions are available with F2.".into(),
                );
                return;
            };
            if src.needs_ticket(w) && !src.is_file(&text) {
                self.status =
                    Some("This workflow checks a ticket. Enter the ticket's file path.".into());
                return;
            }
        }
        let parent = if input == Input::FollowUp {
            self.detail.as_ref().map(|d| d.summary.id.clone())
        } else {
            None
        };
        let agent = self
            .detail
            .as_ref()
            .filter(|_| input == Input::FollowUp)
            .and_then(|d| d.question.as_ref())
            .map(|q| q.agent.clone())
            .unwrap_or_else(|| self.agent.clone());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = if input == Input::Workflow {
                src.start(workflow.as_deref().unwrap_or(""), &text)
            } else {
                src.question_with_agent(parent.as_deref(), &text, &agent)
            };
            let _ = tx.send(result);
        });
        self.pending = Some(rx);
        self.status = Some("Starting…".into());
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(picker) = &mut self.agent_picker {
            if picker.editing_model {
                for c in text.chars().filter(|c| !c.is_control()) {
                    if picker.model.len() + c.len_utf8() > 200 {
                        break;
                    }
                    picker.model.push(c);
                }
            }
            return;
        }
        if self.input.is_some() && self.pending.is_none() {
            let clean: String = text
                .chars()
                .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                .take(32_000usize.saturating_sub(self.text.chars().count()))
                .collect();
            self.text.insert_str(self.cursor, &clean);
            self.cursor += clean.len();
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.pending.is_some() {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('k') {
            self.list_focus = false;
            self.toggle_panel(Panel::Capabilities);
            return;
        }
        if self.panel == Some(Panel::Capabilities) {
            match key.code {
                KeyCode::Esc => self.panel = None,
                KeyCode::Char('r') => self.load_capabilities(),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.item = (self.item + 1).min(self.capabilities.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.item = self.item.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(item) = self.capabilities.get(self.item) {
                        if let Some(why) = &item.unavailable {
                            self.status = Some(why.clone());
                        } else {
                            self.invoke(item.binding.clone(), None);
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(picker) = &mut self.agent_picker {
            if key.code == KeyCode::Esc {
                self.agent_picker = None;
            } else if let Some(selection) = picker.on_key(key) {
                self.agent = selection;
                self.agent_picker = None;
            }
            return;
        }
        if key.code == KeyCode::F(3) {
            if self.detail.is_none() && self.input != Some(Input::Workflow) {
                self.agent_picker = Some(workbench::AgentPicker::new(
                    self.agent.clone(),
                    self.source.as_ref().map(|s| s.agents()).unwrap_or_default(),
                ));
            } else {
                self.status = Some("This task keeps its recorded runtime. Press n for a new question, then F3 to choose its agent.".into());
            }
            return;
        }
        self.sync_clarification();
        if self.input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.keep_clarification_text();
                    self.keep_follow_up();
                    self.input = None;
                    self.list_focus = self.detail.is_none();
                    if self.focused && self.detail.is_none() {
                        self.quit = true;
                    }
                }
                KeyCode::F(2) if matches!(self.input, Some(Input::Question | Input::Workflow)) => {
                    self.input = Some(if self.input == Some(Input::Question) {
                        Input::Workflow
                    } else {
                        Input::Question
                    });
                    self.workflow_focus = self.input == Some(Input::Workflow);
                }
                KeyCode::Tab if self.input == Some(Input::Workflow) => {
                    self.workflow_focus = !self.workflow_focus
                }
                KeyCode::Left | KeyCode::Right
                    if self.workflow_focus && !self.workflows.is_empty() =>
                {
                    let n = self.workflows.len();
                    self.workflow = if key.code == KeyCode::Right {
                        (self.workflow + 1) % n
                    } else {
                        (self.workflow + n - 1) % n
                    };
                }
                KeyCode::Enter if self.workflow_focus => self.workflow_focus = false,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => self.paste("\n"),
                KeyCode::Enter => self.submit(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.workflow_focus = false;
                    self.paste(&c.to_string());
                }
                KeyCode::Backspace if self.cursor > 0 => {
                    let prev = self.text[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.text.drain(prev..self.cursor);
                    self.cursor = prev;
                }
                KeyCode::Delete if self.cursor < self.text.len() => {
                    self.text.remove(self.cursor);
                }
                KeyCode::Left => {
                    self.cursor = self.text[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0)
                }
                KeyCode::Right if self.cursor < self.text.len() => {
                    self.cursor += self.text[self.cursor..].chars().next().unwrap().len_utf8()
                }
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = self.text.len(),
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Char('n') {
            self.compose(Input::Question);
            return;
        }
        if key.code == KeyCode::Char('w') {
            self.compose(Input::Workflow);
            return;
        }
        if key.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        if key.code == KeyCode::Tab {
            self.list_focus = !self.list_focus;
            return;
        }
        if key.code == KeyCode::Esc {
            if self.panel.take().is_none() {
                self.list_focus = !self.list_focus;
            }
            return;
        }
        if self.list_focus {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(self.tasks.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(id) = self.tasks.get(self.selected).map(|t| t.id.clone()) {
                        self.open(&id);
                    }
                }
                _ => {}
            }
            return;
        }
        if self.panel.is_none()
            && !self.text_view
            && let Some((_, request)) = self.clarification_request()
        {
            match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k') => {
                    let active = self.clarification_pending();
                    if let Some(draft) = &mut self.clarification {
                        draft.highlighted =
                            if matches!(key.code, KeyCode::Down | KeyCode::Char('j')) {
                                (draft.highlighted + 1).min(request.options.len())
                            } else {
                                draft.highlighted.saturating_sub(1)
                            };
                        if active {
                            draft.selected = Some(draft.highlighted);
                        }
                    }
                    return;
                }
                KeyCode::Char(' ') if self.clarification_pending() => {
                    if let Some(draft) = &mut self.clarification {
                        draft.selected = Some(draft.highlighted);
                    }
                    return;
                }
                KeyCode::Enter => {
                    if self.clarification_pending() {
                        self.submit_clarification();
                    } else {
                        self.text_view = true;
                    }
                    return;
                }
                KeyCode::Char('f') => {
                    self.edit_clarification(false);
                    return;
                }
                KeyCode::Char('c') => {
                    self.edit_clarification(true);
                    return;
                }
                _ => {}
            }
        }
        let question = self.detail.as_ref().is_some_and(|d| d.question.is_some());
        match key.code {
            KeyCode::Char('h') => self.toggle_panel(Panel::Record),
            KeyCode::Char('b')
                if question
                    && matches!(self.panel, None | Some(Panel::Sources))
                    && self
                        .detail
                        .as_ref()
                        .is_some_and(|d| d.summary.state != State::Working) =>
            {
                let selection = if self.panel == Some(Panel::Sources) {
                    self.current_answer()
                        .and_then(|a| self.source_index().and_then(|i| a.sources.get(i)))
                        .map(|s| {
                            format!(
                                "Source: {}\nURL: {}\nSupporting claim (agent supplied): {}",
                                s.title, s.url, s.supports
                            )
                        })
                } else {
                    self.result_view().map(|v| v.detail(self.result_row))
                };
                if let Some(selection) = selection {
                    let answer_turn = self
                        .answer_turn()
                        .and_then(|answer| {
                            self.detail
                                .as_ref()?
                                .question
                                .as_ref()?
                                .turns
                                .iter()
                                .position(|turn| std::ptr::eq(turn, answer))
                        })
                        .unwrap_or(self.turn);
                    self.compose(Input::FollowUp);
                    if !self.text.is_empty() {
                        self.text.push_str("\n\n");
                    }
                    self.text.push_str(&format!(
                        "About this selection from answer {}:\n{}\n\nMy question: ",
                        answer_turn + 1,
                        clean(&selection)
                    ));
                    self.cursor = self.text.len();
                } else {
                    self.status =
                        Some("Select a fact, table row, chart point, or source first.".into());
                }
            }
            KeyCode::Char('p')
                if !question
                    && self.panel.is_none()
                    && self.detail.as_ref().is_some_and(|d| !d.changes.is_empty()) =>
            {
                self.change_focus = !self.change_focus;
                self.text_view = false;
            }
            KeyCode::Left | KeyCode::Right
                if !question && self.panel.is_none() && !self.text_view =>
            {
                let n = self.detail.as_ref().map_or(0, |d| d.changes.len());
                if n > 0 {
                    self.change = if key.code == KeyCode::Right {
                        (self.change + 1) % n
                    } else {
                        (self.change + n - 1) % n
                    };
                    self.result_row = 0;
                    self.scroll = 0;
                }
            }
            KeyCode::Char('g') if self.agent_available => {
                if let (Some(src), Some(d)) = (&self.source, &self.detail) {
                    self.status = src.open_agent(&d.summary.id).err();
                }
            }
            KeyCode::Char('v') if self.panel.is_none() => {
                self.text_view = !self.text_view;
            }
            KeyCode::Enter if self.panel.is_none() && !self.text_view && self.change_focus => {
                if let Some(change) = self
                    .detail
                    .as_ref()
                    .and_then(|d| d.changes.get(self.change))
                {
                    self.evidence_snapshot = Some((
                        format!(
                            "{} · {} / attempt {}",
                            change.path, change.stage, change.attempt
                        ),
                        change.patch.clone(),
                    ));
                    self.panel = Some(Panel::Evidence);
                    self.expanded = true;
                    self.panel_scroll = 0;
                }
            }
            KeyCode::Enter if self.panel.is_none() && !self.text_view => self.inspect_result(),
            KeyCode::Char('s') if question => self.toggle_panel(Panel::Sources),
            KeyCode::Char('a') => self.toggle_panel(Panel::Activity),
            KeyCode::Char('e') if !question => self.toggle_panel(Panel::Evidence),
            KeyCode::Char('f')
                if question
                    && self
                        .detail
                        .as_ref()
                        .is_some_and(|d| d.summary.state != State::Working) =>
            {
                if self.clarification_pending() {
                    self.edit_clarification(false);
                } else {
                    self.compose(Input::FollowUp);
                }
            }
            KeyCode::Char('r') if self.detail.as_ref().is_some_and(|d| d.native.is_some()) => {
                if let Some(d) = &self.detail {
                    if let Some(binding) = d.native.as_ref().and_then(|n| n.refresh.clone()) {
                        self.invoke(binding, Some(d.summary.id.clone()));
                    } else {
                        self.status = Some("Refresh unavailable. Ctrl+K shows current capabilities; the saved result stays readable.".into());
                    }
                }
            }
            KeyCode::Char('r')
                if question
                    && self
                        .detail
                        .as_ref()
                        .is_some_and(|d| d.summary.state != State::Working) =>
            {
                let text = self
                    .current_turn()
                    .map(|t| t.question.clone())
                    .unwrap_or_default();
                self.compose(Input::FollowUp);
                self.text = text;
                self.cursor = self.text.len();
                self.submit();
            }
            KeyCode::Char('x') if question => {
                if let (Some(src), Some(d)) = (&self.source, &self.detail) {
                    self.status = Some(match src.cancel_question(&d.summary.id) {
                        Ok(()) => "Cancellation requested.".into(),
                        Err(e) => e,
                    });
                }
            }
            KeyCode::Char('c') if self.current_answer().is_some() => {
                if let (Some(src), Some(a)) = (&self.source, self.current_answer()) {
                    self.status = Some(match src.copy(&a.text) {
                        Ok(()) => "Answer copied.".into(),
                        Err(e) => e,
                    });
                }
            }
            KeyCode::Char('y' | 'd') if !question => {
                let yes = key.code == KeyCode::Char('y');
                if let Some(d) = &self.detail
                    && let Some(w) = &d.waiting
                {
                    if let Some(ask) = w.ask {
                        if let Some(src) = &self.source {
                            self.status = Some(match src.answer(&d.summary.id, ask, yes) {
                                Ok(()) => "Answer recorded.".into(),
                                Err(e) => e,
                            });
                        }
                    } else {
                        self.compose(Input::Decision(yes));
                    }
                }
            }
            KeyCode::Char('[' | ']') if question => {
                let len = self
                    .detail
                    .as_ref()
                    .and_then(|d| d.question.as_ref())
                    .map(|q| q.turns.len())
                    .unwrap_or(0);
                self.result_row = 0;
                self.source_filter = None;
                self.selection_detail = None;
                self.turn = if key.code == KeyCode::Char('[') {
                    self.turn.saturating_sub(1)
                } else {
                    (self.turn + 1).min(len.saturating_sub(1))
                };
                self.scroll = 0;
                self.item = 0;
                self.expanded = false;
                self.panel_scroll = 0;
            }
            KeyCode::Enter if self.panel.is_some() && self.panel != Some(Panel::Record) => {
                self.expanded = !self.expanded;
                if self.panel == Some(Panel::Evidence) {
                    self.evidence_snapshot = if self.expanded {
                        self.detail
                            .as_ref()
                            .and_then(|d| d.evidence.get(self.item))
                            .cloned()
                    } else {
                        None
                    };
                }
                self.panel_scroll = 0;
            }
            KeyCode::Char('o') if self.panel == Some(Panel::Sources) => {
                if let (Some(src), Some(c)) = (
                    &self.source,
                    self.current_answer()
                        .and_then(|a| self.source_index().and_then(|i| a.sources.get(i))),
                ) {
                    self.status = src.open_source(&c.url).err();
                }
            }
            KeyCode::Char(c @ '1'..='9') if question => {
                self.source_filter = None;
                self.selection_detail = None;
                self.panel = Some(Panel::Sources);
                self.item = (c as usize - '1' as usize).min(self.items().saturating_sub(1));
                self.expanded = true;
                self.panel_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') if self.panel.is_some() && !self.expanded => {
                self.item = (self.item + 1).min(self.items().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') if self.panel.is_some() && !self.expanded => {
                self.item = self.item.saturating_sub(1)
            }
            KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Up
            | KeyCode::Char('k')
            | KeyCode::PageDown
            | KeyCode::PageUp
                if self.panel.is_none()
                    && !self.text_view
                    && !self.change_focus
                    && self.result_view().is_some() =>
            {
                let len = self.result_view().unwrap().rows.len();
                let step = if matches!(key.code, KeyCode::PageDown | KeyCode::PageUp) {
                    10
                } else {
                    1
                };
                self.result_row = if matches!(
                    key.code,
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown
                ) {
                    (self.result_row + step).min(len.saturating_sub(1))
                } else {
                    self.result_row.saturating_sub(step)
                };
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                let step = if key.code == KeyCode::PageDown { 10 } else { 1 };
                if self.panel.is_some() {
                    self.panel_scroll = self.panel_scroll.saturating_add(step)
                } else {
                    self.scroll = self.scroll.saturating_add(step)
                }
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                let step = if key.code == KeyCode::PageUp { 10 } else { 1 };
                if self.panel.is_some() {
                    self.panel_scroll = self.panel_scroll.saturating_sub(step)
                } else {
                    self.scroll = self.scroll.saturating_sub(step)
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
pub(crate) fn line(text: impl Into<String>, style: Style) -> Line<'static> {
    Line::styled(clean(&text.into()), style)
}
fn state_style(t: &Theme, state: State) -> Style {
    t.fg(match state {
        State::Working => t.accent,
        State::Stopped => t.fail,
        State::NeedsInput => t.warn,
        State::Cancelled => t.dim,
        State::Answered => t.accent,
        _ => t.pass,
    })
}
pub(crate) fn text_block(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>, scroll: u16) {
    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max = p
        .line_count(area.width)
        .saturating_sub(area.height as usize)
        .min(u16::MAX as usize) as u16;
    f.render_widget(p.scroll((scroll.min(max), 0)), area);
}

pub fn draw(f: &mut Frame, app: &Workspace, t: &Theme) {
    let area = f.area();
    f.render_widget(Block::default().style(t.text().bg(t.background)), area);
    let [header, body, status, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(if app.status.is_some() { 2 } else { 0 }),
        Constraint::Length(1),
    ])
    .areas(area);
    let [brand, runtime] = Layout::horizontal([
        Constraint::Min(20),
        Constraint::Length(if area.width >= 100 { 38 } else { 0 }),
    ])
    .areas(header);
    let title = if app.demo {
        " CONDUCTOR / demo · illustrative data"
    } else {
        " CONDUCTOR"
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(title, t.bold()),
            Span::styled(
                if area.width >= 110 {
                    "   n new   w workflow   ^K actions   Tab tasks"
                } else if !app.demo {
                    "   n new   ^K actions   Tab tasks"
                } else {
                    ""
                },
                t.dim(),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(t.fg(t.line)),
        ),
        brand,
    );
    f.render_widget(
        Paragraph::new(clean(&workbench::agent_label(app)))
            .alignment(ratatui::layout::Alignment::Right)
            .style(t.dim())
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(t.fg(t.line)),
            ),
        runtime,
    );
    if let Some(picker) = &app.agent_picker {
        workbench::draw_picker(
            f,
            body.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            }),
            picker,
            t,
        );
    } else if app.panel == Some(Panel::Capabilities) {
        draw_panel(
            f,
            body.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            }),
            app,
            t,
        );
    } else if app.list_focus {
        draw_tasks(f, body, app, t);
    } else {
        let content = body.inner(ratatui::layout::Margin {
            horizontal: 2,
            vertical: u16::from(body.height >= 28),
        });
        if app.detail.is_none() {
            draw_compose(f, content, app, t);
        } else if app.panel.is_some() && content.width < 94 {
            draw_panel(f, content, app, t);
        } else if app.panel.is_some() {
            let [main, panel] =
                Layout::horizontal([Constraint::Min(48), Constraint::Length(43)]).areas(content);
            draw_task(
                f,
                main.inner(ratatui::layout::Margin {
                    horizontal: 1,
                    vertical: 0,
                }),
                app,
                t,
            );
            draw_panel(f, panel, app, t);
        } else {
            draw_task(f, content, app, t);
        }
    }
    if let Some(s) = &app.status {
        f.render_widget(
            Paragraph::new(clean(s))
                .style(t.fg(t.warn))
                .wrap(Wrap { trim: false }),
            status,
        );
    }
    let keys = if app.agent_picker.is_some() {
        " ↑↓ agent   Tab model/mode   Enter select   Esc cancel"
    } else if app.pending.is_some() {
        " Starting…  Ctrl+C quit (work continues)"
    } else if app.panel == Some(Panel::Capabilities) {
        " ↑↓ select   Enter run   r reload capabilities   Esc back"
    } else if matches!(
        app.input,
        Some(Input::ClarificationAnswer | Input::ClarificationContext)
    ) {
        " Enter save/send   Alt+Enter newline   Esc back (keeps draft)"
    } else if app.input.is_some() {
        " Enter send   Alt+Enter newline   Esc back   F2 workflow   F3 agent/model"
    } else if app.list_focus {
        " ↑↓ select   Enter open   n question   w workflow   q quit"
    } else if app.panel.is_some() {
        " ↑↓ select/scroll   Enter expand   PgUp/PgDn scroll   o open source   Esc back"
    } else if app.clarification_request().is_some() && !app.text_view {
        if app.clarification_pending() {
            " ↑↓ choose   Space select   Enter continue   f own answer   c context   Tab tasks"
        } else {
            " Read-only clarification   Enter details   [ previous   ] next   Tab tasks"
        }
    } else if app.change_focus && !app.text_view {
        " ↑↓ scroll   ←→ file   Enter expand patch   p checks   Tab tasks   q quit"
    } else if app.result_view().is_some() && !app.text_view {
        " ↑↓ select   Enter evidence   v full text   Tab tasks   q quit"
    } else {
        " ↑↓ scroll   v view/source   Tab tasks   q close (work continues)"
    };
    f.render_widget(Paragraph::new(keys).style(t.dim()), footer);
}

fn draw_tasks(f: &mut Frame, area: Rect, app: &Workspace, t: &Theme) {
    let block = Block::default()
        .title(if app.list_focus {
            " TASKS · selected "
        } else {
            " TASKS "
        })
        .borders(Borders::RIGHT)
        .border_style(t.fg(t.line))
        .title_style(t.dim());
    let items: Vec<_> = app
        .tasks
        .iter()
        .map(|task| {
            ListItem::new(vec![
                line(task.title.clone(), t.text()),
                line(
                    format!("{} · {}", task.state.label(), task.kind),
                    state_style(t, task.state),
                ),
                Line::raw(""),
            ])
        })
        .collect();
    let mut state = ListState::default().with_selected((!items.is_empty()).then_some(app.selected));
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(t.text().bg(t.sel))
            .highlight_symbol("▎ "),
        area,
        &mut state,
    );
}

fn draw_compose(f: &mut Frame, area: Rect, app: &Workspace, t: &Theme) {
    let width = area.width.min(86);
    let area = Rect::new(area.x, area.y, width, area.height);
    let [intro, input, _] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Min(0),
    ])
    .areas(area);
    let workflow = app.input == Some(Input::Workflow);
    let mut lines = vec![
        line(
            if workflow {
                "Start a workflow"
            } else {
                "What would you like to know?"
            },
            t.bold(),
        ),
        Line::raw(""),
    ];
    if workflow {
        lines.push(line(
            app.workflows
                .get(app.workflow)
                .map(|s| s.rsplit('/').next().unwrap_or(s))
                .unwrap_or("No workflows found"),
            t.fg(t.accent),
        ));
        lines.push(line("Tab select workflow · ← → change", t.dim()));
        lines.push(line("Enter a ticket path or describe the work.", t.text()));
    } else {
        lines.push(line(
            "Ask a question. Follow up in the same task.",
            t.text(),
        ));
        lines.push(line(app.agent.label(), t.fg(t.accent)));
        lines.push(line(app.agent.kind.scope(), t.dim()));
        lines.push(line("F3 choose agent / model", t.dim()));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), intro);
    draw_input(f, input, app, t, " Your request ");
}

fn draw_input(f: &mut Frame, area: Rect, app: &Workspace, t: &Theme, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.fg(if app.workflow_focus { t.line } else { t.accent }))
        .title(title.to_owned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let before = &app.text[..app.cursor];
    let after = &app.text[app.cursor..];
    // A styled cursor wraps with the input, including Unicode and pasted newlines.
    let mut lines: Vec<Line> = before.split('\n').map(|s| Line::raw(clean(s))).collect();
    let current = lines.last_mut().unwrap();
    current.spans.push(Span::styled(
        "▏",
        t.fg(t.accent).add_modifier(Modifier::BOLD),
    ));
    let mut rest = after.split('\n');
    current
        .spans
        .push(Span::raw(clean(rest.next().unwrap_or(""))));
    lines.extend(rest.map(|s| Line::raw(clean(s))));
    let text = ratatui::text::Text::from(lines);
    let p = Paragraph::new(text)
        .style(t.text())
        .wrap(Wrap { trim: false });
    let cursor_lines = Paragraph::new(clean(before))
        .wrap(Wrap { trim: false })
        .line_count(inner.width);
    let scroll = cursor_lines
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    f.render_widget(p.scroll((scroll, 0)), inner);
}

fn draw_clarification(
    f: &mut Frame,
    area: Rect,
    app: &Workspace,
    binding: &InputBinding,
    request: &InputRequest,
    t: &Theme,
) {
    let Some(d) = &app.detail else {
        return;
    };
    let fallback = clarification::Draft::new(binding.clone());
    let draft = app
        .clarification
        .as_ref()
        .filter(|d| d.binding == *binding)
        .unwrap_or(&fallback);
    let editing = matches!(
        app.input,
        Some(Input::ClarificationAnswer | Input::ClarificationContext)
    );
    let active = app.clarification_pending();
    let q = d.question.as_ref().unwrap();
    let response = q
        .turns
        .iter()
        .filter_map(|t| t.input_response.as_ref())
        .find(|r| r.binding == *binding);
    let actions = if editing { 6 } else { 3 };
    let height =
        clarification::height(request, area.width, t).min(area.height.saturating_sub(actions + 4));
    let [header, card, action, _] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(height),
        Constraint::Length(actions),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(vec![
            line(&d.summary.title, t.bold()),
            line(
                if active {
                    "Needs your answer".into()
                } else {
                    format!(
                        "Earlier question · turn {} of {} · ] next",
                        binding.turn,
                        q.turns.len()
                    )
                },
                if active { t.fg(t.warn) } else { t.dim() },
            ),
            Line::raw(""),
        ])
        .wrap(Wrap { trim: false }),
        header,
    );
    clarification::draw(f, card, request, draft, active, response, t);
    if editing {
        draw_input(
            f,
            action,
            app,
            t,
            if app.input == Some(Input::ClarificationContext) {
                " Add context · Enter save · Esc back "
            } else {
                " Your answer · Enter continue · Esc back "
            },
        );
    } else {
        f.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                line(
                    if active {
                        "Enter Continue   f My own answer   c Add context"
                    } else {
                        "Enter Response details   ] Next turn"
                    },
                    t.fg(t.accent),
                ),
                line("a Activity   v Original message", t.dim()),
            ]),
            action,
        );
    }
}

fn draw_task(f: &mut Frame, area: Rect, app: &Workspace, t: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(d) = &app.detail else { return };
    if !app.text_view
        && let Some((binding, request)) = app.clarification_request()
    {
        draw_clarification(f, area, app, &binding, &request, t);
        return;
    }
    let question = d.question.is_some();
    let state = app
        .current_turn()
        .map(|q| q.state)
        .unwrap_or(d.summary.state);
    let action_height =
        if area.height >= 18 && (question || app.input.is_some() || d.waiting.is_some()) {
            5
        } else {
            2
        };
    let header_height = if area.height >= 18 {
        5
    } else if area.height >= 12 {
        3
    } else {
        2
    };
    let available = area
        .height
        .saturating_sub(header_height + action_height + 1);
    let view = app.result_view().filter(|_| !app.text_view);
    let mut context = Vec::new();
    if let Some(notice) = d.native.as_ref().and_then(|n| n.notice.as_ref()) {
        context.push(line(notice, t.fg(t.warn)));
    }
    if let Some(q) = &d.question {
        if q.turns.len() > 1 {
            context.push(line(
                format!(
                    "Answer {} of {} · [ previous · ] next",
                    app.turn + 1,
                    q.turns.len()
                ),
                t.dim(),
            ));
        }
        if let Some(turn) = app.current_turn() {
            if turn.input_response.is_some() {
                context.push(line(&turn.question, t.dim()));
            }
            if let Some(error) = &turn.error {
                context.push(line(error, t.fg(t.fail)));
            }
            if app.answer_is_previous() {
                context.push(line(
                    "Previous answer · the latest request has no new answer yet.",
                    t.fg(t.warn),
                ));
            }
            if state == State::Working {
                context.push(line(
                    turn.activity
                        .last()
                        .map(activity_title)
                        .unwrap_or_else(|| "Starting the lookup…".into()),
                    t.fg(t.accent),
                ));
            }
        }
    }
    if let Some(w) = &d.waiting {
        context.push(line(&w.question, t.fg(t.warn)));
    }
    let context_height = Paragraph::new(context.clone())
        .wrap(Wrap { trim: false })
        .line_count(area.width)
        .min(6)
        .min(available as usize) as u16;
    let document_view = view.is_none() && (question || d.native.is_some());
    let text_width = if document_view {
        document::inner_width(area.width)
    } else {
        area.width
    };
    let text = task_text(app, t, text_width);
    let has_changes = !question && !d.changes.is_empty() && !app.text_view;
    let desired = if has_changes {
        22
    } else if let Some(v) = &view {
        result_view::height(v, area.width)
    } else {
        Paragraph::new(text.clone())
            .wrap(Wrap { trim: false })
            .line_count(text_width)
            .min(u16::MAX as usize) as u16
            + if document_view { 2 } else { 0 }
    };
    let content_height = desired.saturating_add(context_height).min(available);
    let [header, body, _, gap, action] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Length(content_height),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(action_height),
    ])
    .areas(area);
    let title = app
        .current_turn()
        .filter(|turn| turn.input_response.is_none())
        .map(|q| q.question.as_str())
        .unwrap_or(&d.summary.title);
    let [request, status, toolbar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(header);
    f.render_widget(
        Paragraph::new(clean(title))
            .style(t.bold())
            .wrap(Wrap { trim: false }),
        request,
    );
    let label = if !question && d.waiting.as_ref().is_some_and(|w| w.ask.is_none()) {
        "Ready for review"
    } else {
        state.label()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, state_style(t, state)),
            Span::styled(format!(" · {}", clean(&d.context)), t.dim()),
        ]))
        .wrap(Wrap { trim: false }),
        status,
    );
    if toolbar.height >= 2 {
        let mut tools = if question {
            "s Sources   a Activity   h Source record".to_owned()
        } else {
            "e Evidence   a Activity   h Receipt".to_owned()
        };
        if app.agent_available {
            tools.push_str("   g Open agent");
        }
        f.render_widget(
            Paragraph::new(vec![Line::raw(""), line(tools, t.fg(t.accent))]),
            toolbar,
        );
    }
    let [notice, result] =
        Layout::vertical([Constraint::Length(context_height), Constraint::Min(0)]).areas(body);
    text_block(f, notice, context, 0);
    if has_changes {
        draw_change(f, result, app, view.as_ref(), t);
    } else if let Some(view) = &view {
        result_view::draw(f, result, view, app.result_row, t);
    } else if document_view {
        let title = if let Some(native) = &d.native {
            match native.media_type.as_str() {
                "text/markdown" if app.text_view => "Document · Markdown source",
                "text/markdown" => "Document · Markdown",
                "application/json" => "Data · JSON",
                _ => "Document · Text",
            }
        } else if app.text_view && app.result_view().is_none() {
            "Answer · Markdown source"
        } else {
            "Answer · Markdown"
        };
        document::draw(f, result, text, app.scroll, title, t);
    } else {
        text_block(f, result, text, app.scroll);
    }
    f.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(t.fg(t.line)),
        gap,
    );
    if app.input.is_some() {
        draw_input(
            f,
            action,
            app,
            t,
            match app.input {
                Some(Input::Decision(true)) => " What did you check? ",
                Some(Input::Decision(false)) => " What needs to change? ",
                _ => " Follow up · Enter send ",
            },
        );
        return;
    }
    let evidence = if question { "s Sources" } else { "e Evidence" };
    let mut commands = format!(
        "{evidence}   a Activity   h {}",
        if question { "Source record" } else { "Receipt" }
    );
    if app.agent_available {
        commands.push_str("   g Open agent");
    }
    if has_changes {
        commands.push_str("   p Changes / checks");
    }
    let mut actions = vec![line(
        if header_height >= 5 {
            if has_changes {
                "p Changes / checks   ← → file   Enter inspect".into()
            } else if question {
                format!(
                    "{}   {}",
                    workbench::agent_label(app),
                    if app.current_answer().is_some() {
                        "b Ask about selection"
                    } else {
                        ""
                    }
                )
            } else {
                commands.clone()
            }
        } else {
            commands
        },
        t.dim(),
    )];
    if question {
        actions.push(line(
            if d.summary.state == State::Working {
                "x Cancel lookup"
            } else if matches!(state, State::Stopped | State::Cancelled) {
                "f Follow up   r Retry   c Copy"
            } else {
                "f Follow up   r Refresh   c Copy"
            },
            t.text(),
        ));
    } else if let Some(native) = &d.native {
        actions.push(line(
            if native.refresh.is_some() {
                "r Refresh   c Copy   ^K Actions"
            } else {
                "c Copy   ^K Actions · refresh unavailable"
            },
            t.text(),
        ));
    } else if let Some(w) = &d.waiting {
        actions.push(line(
            if w.ask.is_some() {
                "y Allow tool   d Deny tool"
            } else {
                "y Accept review   d Reject with a note"
            },
            t.text(),
        ));
    } else {
        actions.push(line(
            "v Full record · checks describe the saved attempt",
            t.dim(),
        ));
    }
    let [keys, input] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(action);
    f.render_widget(Paragraph::new(actions), keys);
    if !question && d.waiting.is_some() && input.height >= 2 {
        f.render_widget(
            Paragraph::new(if let Some(receipt) = &d.receipt {
                if receipt.not_checked.is_empty() {
                    "Review applies to the recorded checks only.".into()
                } else {
                    format!("Not checked: {}", clean(&receipt.not_checked.join(" · ")))
                }
            } else {
                "Final receipt pending · e inspect recorded checks before deciding".into()
            })
            .style(t.dim())
            .wrap(Wrap { trim: false }),
            input,
        );
    }
    if question && d.summary.state != State::Working && input.height >= 2 {
        f.render_widget(
            Paragraph::new(" f  Ask about this answer…")
                .style(t.dim())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(t.text().bg(t.surface))
                        .border_style(t.fg(t.line)),
                ),
            input,
        );
    }
}

fn task_text(app: &Workspace, t: &Theme, width: u16) -> Vec<Line<'static>> {
    let Some(d) = &app.detail else { return vec![] };
    if let Some(native) = &d.native {
        let text = app
            .current_answer()
            .map(|a| a.text.as_str())
            .unwrap_or(&d.body);
        return if native.media_type == "text/markdown" && !app.text_view {
            document::lines(text, width, t, &[])
        } else {
            document::plain(text, width, t)
        };
    }
    if d.question.is_none() {
        return document::lines(&d.body, width, t, &[]);
    }
    if let Some(answer) = app.current_answer() {
        if app.text_view && app.result_view().is_none() {
            let mut lines = document::plain(&answer.text, width, t);
            if let Some((binding, _)) = app.clarification_request()
                && let Some(turn) = d.question.as_ref().and_then(|q| {
                    q.turns.iter().find(|turn| {
                        turn.input_response
                            .as_ref()
                            .is_some_and(|r| r.binding == binding)
                    })
                })
            {
                lines.push(Line::raw(""));
                lines.push(line("Recorded response", t.bold()));
                lines.extend(document::plain(&turn.question, width, t));
            }
            return lines;
        }
        let mut lines = vec![];
        if let Err(why) = answer.input_request() {
            lines.extend(document::plain(why, width, t));
        }
        if let Err(why) = answer.presentation() {
            lines.extend(document::plain(why, width, t));
        }
        if app.text_view
            && let Ok(Some(p)) = answer.presentation()
        {
            let (title, summary) = match p {
                Presentation::Facts { title, summary, .. }
                | Presentation::Table { title, summary, .. }
                | Presentation::Series { title, summary, .. } => (title, summary),
            };
            lines.extend(document::lines(
                &format!("## {title}\n\n{summary}"),
                width,
                t,
                &answer.sources,
            ));
            lines.push(Line::raw(""));
        }
        lines.extend(document::lines(&answer.text, width, t, &answer.sources));
        if answer.needs_input && answer.sources.is_empty() {
            return lines;
        }
        lines.push(Line::raw(""));
        lines.extend(document::wrap(
            &[Span::styled(
                if answer.sources.is_empty() {
                    "No sources cited.".into()
                } else {
                    format!(
                        "{} source{} · s inspect",
                        answer.sources.len(),
                        if answer.sources.len() == 1 { "" } else { "s" }
                    )
                },
                t.dim(),
            )],
            width,
            "",
            "",
            true,
        ));
        lines
    } else if app
        .current_turn()
        .is_some_and(|q| q.state == State::Cancelled)
    {
        vec![line(
            "Cancelled. Your question and activity are saved.",
            t.text(),
        )]
    } else {
        vec![]
    }
}

fn draw_change(f: &mut Frame, area: Rect, app: &Workspace, checks: Option<&View>, t: &Theme) {
    let Some(d) = &app.detail else { return };
    let Some(change) = d.changes.get(app.change) else {
        return;
    };
    let [heading, content] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(vec![
            line(
                format!(
                    "Saved changes · {} / attempt {}",
                    change.stage, change.attempt
                ),
                t.bold(),
            ),
            line(
                format!(
                    "{}  ·  file {} of {}  ·  ← → choose file",
                    change.path,
                    app.change + 1,
                    d.changes.len()
                ),
                t.fg(t.accent),
            ),
        ])
        .wrap(Wrap { trim: false }),
        heading,
    );
    let (patch, report) = if content.width >= 96 {
        let [patch, report] =
            Layout::horizontal([Constraint::Percentage(57), Constraint::Percentage(43)])
                .areas(content);
        (
            Some(patch),
            Some(report.inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 0,
            })),
        )
    } else if app.change_focus {
        (Some(content), None)
    } else {
        (None, Some(content))
    };
    if let Some(area) = patch {
        let block = Block::default()
            .title(if app.change_focus {
                " Change · selected "
            } else {
                " Change "
            })
            .title_style(t.dim())
            .borders(Borders::ALL)
            .border_style(t.fg(if app.change_focus { t.accent } else { t.line }));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let lines = change
            .patch
            .lines()
            .map(|s| {
                line(
                    s,
                    if s.starts_with('+') && !s.starts_with("+++") {
                        t.fg(t.pass).bg(t.pass_bg)
                    } else if s.starts_with('-') && !s.starts_with("---") {
                        t.fg(t.fail).bg(t.fail_bg)
                    } else if s.starts_with("@@") {
                        t.fg(t.accent)
                    } else {
                        t.dim()
                    },
                )
            })
            .collect();
        text_block(f, inner, lines, app.scroll);
    }
    if let Some(area) = report {
        let block = Block::default()
            .title(if !app.change_focus {
                " Checks · selected "
            } else {
                " Checks "
            })
            .title_style(t.dim())
            .borders(Borders::ALL)
            .border_style(t.fg(if !app.change_focus { t.accent } else { t.line }));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let area = inner;
        if let Some(view) = checks {
            result_view::draw_checks(f, area, view, app.result_row, t);
        } else {
            text_block(
                f,
                area,
                vec![
                    line("Checks", t.bold()),
                    Line::raw(""),
                    line("No parsed test cases for this saved attempt.", t.fg(t.warn)),
                    line("e Evidence shows recorded gates and limitations.", t.dim()),
                    line("v Full record · p Changes", t.dim()),
                ],
                0,
            );
        }
    }
}

fn activity_title(a: &Activity) -> String {
    match a.title.as_str() {
        "WebFetch returned" => "Source retrieved".into(),
        "WebFetch failed" => "Source retrieval failed".into(),
        "WebSearch returned" => "Search results received".into(),
        "WebSearch failed" => "Source search failed".into(),
        "Using StructuredOutput" => "Preparing the answer".into(),
        "StructuredOutput returned" => "Answer prepared".into(),
        _ => a.title.clone(),
    }
}

fn captured_text(text: &str, t: &Theme, width: u16) -> Vec<Line<'static>> {
    // Evidence and tool output retain their original line breaks and punctuation.
    document::plain(text, width, t)
}

fn draw_panel(f: &mut Frame, area: Rect, app: &Workspace, t: &Theme) {
    let Some(panel) = app.panel else { return };
    if panel == Panel::Record {
        workbench::draw_record(f, area, app, t);
        return;
    }
    let title = match panel {
        Panel::Record => " Record ",
        Panel::Capabilities => " Actions · discovered capabilities ",
        Panel::Sources if app.source_filter.is_some() => " Sources · selected row ",
        Panel::Sources => " Sources ",
        Panel::Activity => " Activity ",
        Panel::Evidence => " Evidence ",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::LEFT | Borders::TOP)
        .border_style(t.fg(t.line))
        .title_style(t.bold());
    let inner = block.inner(area).inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 0,
    });
    f.render_widget(block, area);
    let [hint, content] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
    f.render_widget(
        Paragraph::new(if panel == Panel::Capabilities {
            "Enter run · r reload · Esc back"
        } else {
            "Esc back · Enter expand"
        })
        .style(t.dim()),
        hint,
    );
    let mut rows: Vec<(String, String, bool)> = vec![];
    match panel {
        Panel::Record => {}
        Panel::Capabilities => {
            for item in &app.capabilities {
                rows.push((
                    item.label.clone(),
                    item.unavailable
                        .as_ref()
                        .map(|e| format!("Unavailable: {e}"))
                        .unwrap_or_else(|| format!("{} · {}", item.binding.id, item.detail)),
                    item.unavailable.is_some(),
                ));
            }
        }
        Panel::Sources => {
            if let Some(answer) = app.current_answer() {
                for (i, c) in answer.sources.iter().enumerate() {
                    if app
                        .source_filter
                        .as_ref()
                        .is_some_and(|refs| !refs.contains(&i))
                    {
                        continue;
                    }
                    let mut detail = format!("{}\n\nSupports\n{}", c.url, c.supports);
                    if let Some(selected) = &app.selection_detail {
                        detail = format!("{selected}\n\n{detail}");
                    }
                    if let Some(at) = &c.observed_at {
                        detail.push_str(&format!("\n\nObserved (source, as cited)\n{at}"));
                    }
                    if let Some(at) = &c.retrieved_at {
                        detail.push_str(&format!("\n\nRetrieved by tool\n{at}"));
                    }
                    detail.push_str("\n\nCitation supplied by agent.");
                    if let Some(captured) = &c.captured {
                        detail.push_str(&format!("\n\nCaptured tool response\n{captured}"));
                    } else {
                        detail.push_str(" No matching fetch captured.");
                    }
                    rows.push((format!("[{}] {}", i + 1, c.title), detail, false));
                }
            }
        }
        Panel::Activity => {
            for a in app.activity().iter().rev() {
                rows.push((
                    format!("{}{}", if a.failed { "! " } else { "" }, activity_title(a)),
                    format!("{} · {}\n\n{}", a.at, a.actor, a.detail),
                    a.failed,
                ));
            }
        }
        Panel::Evidence => {
            if let Some((title, detail)) = &app.evidence_snapshot {
                let mut lines = vec![
                    line(title, t.bold()),
                    line("Saved selection · original capture retained below", t.dim()),
                    Line::raw(""),
                ];
                lines.extend(captured_text(detail, t, content.width));
                text_block(f, content, lines, app.panel_scroll);
                return;
            }
            if let Some(d) = &app.detail {
                for (title, detail) in &d.evidence {
                    rows.push((title.clone(), detail.clone(), false));
                }
            }
        }
    }
    if rows.is_empty() {
        let mut lines = captured_text(
            app.selection_detail.as_deref().unwrap_or(""),
            t,
            content.width,
        );
        lines.push(Line::raw(""));
        lines.push(line(
            match panel {
                Panel::Record => "No record selected.",
                Panel::Capabilities => "No operations available. Questions remain available with n. Register reports with conductor capability add.",
                Panel::Sources if app.source_filter.is_some() => {
                    "No source attached to this row. s shows all answer sources."
                }
                Panel::Sources => "No sources cited for this answer.",
                Panel::Activity => "No activity recorded yet.",
                Panel::Evidence => "No evidence recorded yet.",
            },
            t.dim(),
        ));
        text_block(f, content, lines, app.panel_scroll);
    } else if app.expanded {
        if let Some((title, detail, failed)) = rows.get(app.item) {
            let mut lines = vec![
                line(title, if *failed { t.fg(t.fail) } else { t.bold() }),
                Line::raw(""),
            ];
            lines.extend(captured_text(detail, t, content.width));
            text_block(f, content, lines, app.panel_scroll);
        }
    } else {
        let items: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(index, (title, detail, failed))| {
                ListItem::new(vec![
                    line(title, if *failed { t.fg(t.fail) } else { t.text() }),
                    line(
                        if panel == Panel::Activity {
                            app.activity()
                                .get(app.activity().len().saturating_sub(index + 1))
                                .map(|a| a.actor.as_str())
                                .unwrap_or("")
                        } else {
                            detail.lines().next().unwrap_or("")
                        },
                        t.dim(),
                    ),
                    Line::raw(""),
                ])
            })
            .collect();
        let mut selection = ListState::default().with_selected(Some(app.item.min(rows.len() - 1)));
        f.render_stateful_widget(
            List::new(items)
                .highlight_style(t.text().bg(t.sel))
                .highlight_symbol("▎ "),
            content,
            &mut selection,
        );
    }
}

/// Stable demo data, deliberately labelled as illustrative rather than current weather.
pub fn demo_task() -> TaskDetail {
    let summary = TaskSummary {
        id: "q-demo".into(),
        title: "Weather in Los Angeles".into(),
        kind: "Question".into(),
        state: State::Answered,
    };
    let answer = Answer {text:"## Los Angeles, California\n\n24°C / 75°F · Clear\nFeels like 25°C / 77°F\n\nObserved 26 Sep 2026, 10:00 PDT [1]\n\nIllustrative demo data — not a current weather report.".into(),sources:vec![Citation {title:"Example weather provider".into(),url:"https://example.com/weather".into(),supports:"Temperature, conditions and observation time.".into(),observed_at:Some("26 Sep 2026, 10:00 PDT".into()),retrieved_at:Some("2026-09-26T17:02:00Z".into()),captured:Some("Demo response: temperature 24°C; clear; station Los Angeles.".into())}],needs_input:false, input_request:None, presentation:None};
    let answer = answer.with_presentation(Presentation::Facts {
        title: "Los Angeles, California".into(),
        summary: "Observed 26 Sep 2026, 10:00 PDT · Example weather provider. Illustrative demo data — not a current weather report.".into(),
        facts: vec![
            Fact { label: "Temperature".into(), value: "24°C / 75°F".into(), sources: vec![1] },
            Fact { label: "Conditions".into(), value: "Clear".into(), sources: vec![1] },
            Fact { label: "Feels like".into(), value: "25°C / 77°F".into(), sources: vec![1] },
        ],
    });
    let activity = vec![
        Activity {
            at: "2026-09-26T17:01:59Z".into(),
            actor: "You".into(),
            title: "Question submitted".into(),
            detail: "What is the weather in LA?".into(),
            failed: false,
        },
        Activity {
            at: "2026-09-26T17:02:00Z".into(),
            actor: "Tool".into(),
            title: "Weather source returned".into(),
            detail: "Illustrative response from an example provider.".into(),
            failed: false,
        },
        Activity {
            at: "2026-09-26T17:02:01Z".into(),
            actor: "Conductor".into(),
            title: "Answer returned".into(),
            detail: "Sources attached; factual accuracy is not independently verified.".into(),
            failed: false,
        },
    ];
    TaskDetail {
        receipt: None,
        native: None,
        summary,
        context: "Web question · No project files supplied".into(),
        body: String::new(),
        evidence: vec![],
        activity: vec![],
        waiting: None,
        tests: None,
        changes: vec![],
        question: Some(Question {
            agent: Default::default(),
            id: "q-demo".into(),
            title: "Weather in Los Angeles".into(),
            pid: None,
            turns: vec![Turn {
                input_response: None,
                question: "What is the weather in LA?".into(),
                started_at: "2026-09-26T17:01:59Z".into(),
                state: State::Answered,
                answer: Some(answer),
                activity,
                error: None,
                tokens: None,
                cost_usd: None,
            }],
        }),
    }
}

/// Illustrative comparison and forecast views, available in the interactive demo task list.
pub fn demo_comparison(forecast: bool) -> TaskDetail {
    let mut d = demo_task();
    let (id, title, columns, values) = if forecast {
        (
            "q-forecast",
            "What about tomorrow in LA?",
            vec!["Time", "Conditions", "Temperature"],
            vec![
                vec!["Morning", "Clear", "19°C / 66°F"],
                vec!["Afternoon", "Sunny", "27°C / 81°F"],
                vec!["Evening", "Partly cloudy", "22°C / 72°F"],
            ],
        )
    } else {
        (
            "q-comparison",
            "Compare LA and San Francisco",
            vec!["City", "Conditions", "Temperature"],
            vec![
                vec!["Los Angeles", "Clear", "24°C / 75°F"],
                vec!["San Francisco", "Fog", "16°C / 61°F"],
            ],
        )
    };
    d.summary.id = id.into();
    d.summary.title = title.into();
    let q = d.question.as_mut().unwrap();
    q.id = id.into();
    q.title = title.into();
    let turn = &mut q.turns[0];
    turn.question = title.into();
    let columns: Vec<String> = columns.into_iter().map(str::to_owned).collect();
    let rows: Vec<ResultRow> = values
        .into_iter()
        .map(|cells| ResultRow {
            cells: cells.into_iter().map(str::to_owned).collect(),
            sources: vec![1],
        })
        .collect();
    let text = format!(
        "## {title}\nIllustrative demo data — not current weather.\n{}",
        rows.iter()
            .map(|r| columns
                .iter()
                .zip(&r.cells)
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(" · "))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut answer = turn.answer.take().unwrap();
    answer.text = text;
    answer.sources[0].supports = "Illustrative values in this demo table.".into();
    answer.sources[0].captured =
        Some("Illustrative fixture only; no weather lookup was performed.".into());
    turn.answer = Some(answer.with_presentation(Presentation::Table {
        title: if forecast { "Tomorrow · Los Angeles" } else { "Current conditions · city comparison" }.into(),
        summary: "Illustrative demo data — not a current weather report. Example provider; 26 Sep 2026.".into(), columns, rows
    }));
    d
}

pub fn demo_tests() -> TaskDetail {
    let mut d = demo_task();
    d.summary = TaskSummary {
        id: "test-demo".into(),
        title: "QA · account ID normalization".into(),
        kind: "QA".into(),
        state: State::Stopped,
    };
    d.question = None;
    d.context = "Illustrative test report · No commands executed".into();
    d.body = "## QA result\nThe lowercase account ID test failed. Two cases passed; one failed.\nIllustrative fixture only; no commands were executed.".into();
    d.evidence = vec![
        ("Failed · lowercase_account_id".into(), "Captured test message (illustrative):\nassertion failed: balance request accepts lowercase account ID\nexpected status: 200\nactual status: 404\n\nStage: test · attempt 1\nCommand: cargo test\nExit: 101".into()),
        ("Passed · uppercase_account_id".into(), "Illustrative test case passed. No per-test output captured.".into()),
        ("Passed · missing_account_id".into(), "Illustrative test case passed. No per-test output captured.".into()),
    ];
    d.tests = Some(TestResults {
        notices: vec!["test · attempt 1 · gate failed · illustrative fixture".into()],
        cases: [
            "lowercase_account_id",
            "uppercase_account_id",
            "missing_account_id",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, name)| TestResult {
            id: name.into(),
            name: name.into(),
            outcome: if i == 0 { "Failed" } else { "Passed" }.into(),
            stage: "test".into(),
            attempt: 1,
            evidence: i,
        })
        .collect(),
    });
    d
}

/// The approved bug-fix layout, with explicitly illustrative saved patches and checks.
pub fn demo_bug() -> TaskDetail {
    use conductor_model::task::TaskChange;
    let mut d = demo_tests();
    d.summary.id = "bug-demo".into();
    d.summary.title = "BUG-102 · Lowercase account IDs fail".into();
    d.summary.kind = "Bug fix".into();
    d.summary.state = State::NeedsInput;
    d.context = "Illustrative data · fix / attempt 2 · not merged".into();
    d.waiting = Some(conductor_model::view::Waiting {
        stage: "review".into(),
        who: "You".into(),
        question: "Review the normalization change and its recorded checks.".into(),
        since: "2026-09-27T00:00:00Z".into(),
        ask: None,
    });
    d.body = "## Saved change\nNormalize account IDs before lookup, preserving validation.\nIllustrative fixture; no changes or checks were actually executed.".into();
    d.evidence.clear();
    let names = [
        "lowercase_id_resolves",
        "mixed_case_id_resolves",
        "unknown_id_stays_404",
        "whitespace_stays_invalid",
    ];
    d.tests = Some(TestResults {
        cases: names.into_iter().enumerate().map(|(i, name)| {
            d.evidence.push((name.into(), format!("Illustrative recorded result: Passed\nStage: fix · attempt 2\nCheck: {name}\nNo real commands executed.")));
            TestResult { id: format!("fix/2/{name}"), name: name.into(), outcome: "Passed".into(), stage: "fix".into(), attempt: 2, evidence: i }
        }).collect(),
        notices: vec!["fix · attempt 2 · gate passed · illustrative fixture".into()],
    });
    d.changes = vec![
        TaskChange { stage: "fix".into(), attempt: 2, path: "src/accounts/lookup.rs".into(),
            patch: "diff --git a/src/accounts/lookup.rs b/src/accounts/lookup.rs\n@@ lookup @@\n-validate_id(id)?;\n-store.find(id)\n+let canonical = id.to_ascii_uppercase();\n+validate_id(&canonical)?;\n+store.find(&canonical)\n".into() },
        TaskChange { stage: "fix".into(), attempt: 2, path: "tests/lookup_test.rs".into(),
            patch: "diff --git a/tests/lookup_test.rs b/tests/lookup_test.rs\n@@ regression cases @@\n+lowercase_id_resolves()\n+mixed_case_id_resolves()\n unknown_id_stays_404()\n whitespace_stays_invalid()\n".into() },
    ];
    d
}

/// Illustrative clarification. Selecting an option in --demo never starts an agent.
pub fn demo_clarification() -> TaskDetail {
    let mut d = demo_task();
    d.summary.id = "q-clarification".into();
    d.summary.title = "Best resources to study FDE".into();
    d.summary.state = State::NeedsInput;
    let q = d.question.as_mut().unwrap();
    q.id = d.summary.id.clone();
    q.title = d.summary.title.clone();
    let turn = &mut q.turns[0];
    turn.question = q.title.clone();
    turn.state = State::NeedsInput;
    turn.answer=Some(Answer {
        text:"FDE can mean different subjects. What does FDE mean here?\n\n- Forward Deployed Engineer: customer-facing engineering.\n- Full Disk Encryption: disk security.\n- Fundamentals of Data Engineering: the book or subject.\n\nYou can write your own interpretation.".into(),
        ..Answer::default()
    }.with_input_request(InputRequest {
        kind:conductor_model::interaction::InputKind::SingleChoice,
        question:"What does FDE mean here?".into(),
        explanation:"The study resources depend on which subject you mean.".into(),
        options:[("fde","Forward Deployed Engineer","Customer-facing engineering and implementation."),("encryption","Full Disk Encryption","BitLocker, FileVault, LUKS and security."),("data","Fundamentals of Data Engineering","The book or the broader subject.")].into_iter().map(|(id,label,description)|conductor_model::interaction::Choice{id:id.into(),label:label.into(),description:description.into()}).collect(),
    }));
    turn.activity = vec![Activity {
        at: "2026-09-27T10:00:00Z".into(),
        actor: "Agent".into(),
        title: "Clarification requested".into(),
        detail: "Illustrative question. No agent was invoked.".into(),
        failed: false,
    }];
    d
}

/// A generic numeric series: the renderer has no weather-specific logic.
pub fn demo_series() -> TaskDetail {
    let mut d = demo_task();
    d.summary.id = "q-series".into();
    d.summary.title = "When should I run in LA tomorrow?".into();
    let q = d.question.as_mut().unwrap();
    q.id = d.summary.id.clone();
    q.title = d.summary.title.clone();
    let turn = &mut q.turns[0];
    turn.question = q.title.clone();
    let mut answer = turn.answer.take().unwrap();
    answer.sources[0].title = "Example hourly forecast".into();
    answer.sources[0].url = "https://example.com/forecast".into();
    answer.sources[0].supports = "Illustrative forecast values for the displayed periods.".into();
    answer.sources[0].observed_at = Some("Fixture forecast for 27 Sep 2026".into());
    answer.sources[0].captured = Some("Illustrative forecast: 06:00 17°C; 09:00 21°C; 12:00 27°C; 15:00 29°C; 18:00 24°C. No live request was made.".into());
    turn.answer = Some(Answer { text: "## Morning is the cooler window\n\nIllustrative forecast for Los Angeles; not live weather.\n\n06:00: 17°C; 09:00: 21°C; 12:00: 27°C; 15:00: 29°C; 18:00: 24°C. [1]".into(), ..answer }.with_presentation(Presentation::Series {
        title: "Morning is the cooler window".into(),
        summary: "Los Angeles · tomorrow · illustrative forecast, not current weather. Select a period to inspect the values and source.".into(), unit: "°C".into(),
        points: [("06:00",17.,"Cooler start · light wind"),("09:00",21.,"Mild · sun rising"),("12:00",27.,"Warm · exposed routes feel hotter"),("15:00",29.,"Warmest sample period"),("18:00",24.,"Cooling into the evening")].into_iter().map(|(label,value,detail)| conductor_model::task::SeriesPoint { label: label.into(), value, detail: detail.into(), sources: vec![1] }).collect(),
    }));
    d
}
