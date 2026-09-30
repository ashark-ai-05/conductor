use conductor_model::task::{State, TaskDetail, TaskSummary};
use conductor_tui::{
    app::RunSource,
    theme::Theme,
    workspace::{self, Input, Panel, TaskSource, Workspace},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use std::sync::{Arc, Mutex};

fn press(app: &mut Workspace, key: KeyCode) {
    app.on_key(KeyEvent::new(key, KeyModifiers::NONE));
}
fn render(app: &Workspace, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| workspace::draw(f, app, &Theme::DARK))
        .unwrap();
    let b = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| b[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn answer_and_sources_are_together_wide_and_back_restores_the_answer_when_narrow() {
    let mut app = Workspace::demo();
    let answer = render(&app, 140, 32);
    assert!(answer.contains("24°C / 75°F"));
    assert!(answer.contains("Answered"));
    assert!(!answer.contains("Accept"));
    press(&mut app, KeyCode::Char('1'));
    let wide = render(&app, 140, 32);
    assert!(wide.contains("24°C / 75°F"));
    assert!(wide.contains("Supports"));
    assert!(wide.contains("Captured tool response"));
    let narrow = render(&app, 75, 32);
    assert!(narrow.contains("Captured tool response"));
    assert!(!narrow.contains("24°C / 75°F"));
    press(&mut app, KeyCode::Esc);
    assert!(render(&app, 75, 32).contains("24°C / 75°F"));
}

#[test]
fn typing_shortcut_characters_and_unicode_does_not_navigate() {
    let mut app = Workspace::demo();
    press(&mut app, KeyCode::Char('f'));
    app.paste("q1rs météo\nLA");
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('é'));
    assert_eq!(app.text, "q1rs météo\nLéA");
    assert_eq!(app.input, Some(Input::FollowUp));
    assert!(!app.quit);
    assert_eq!(app.panel, None);
    let output = render(&app, 80, 30);
    assert!(output.contains("q1rs météo"));
    assert!(output.contains("Lé▏A"));
}

#[test]
fn inspectors_and_input_survive_small_terminal_sizes() {
    let mut app = Workspace::demo();
    for panel in [None, Some(Panel::Sources), Some(Panel::Activity)] {
        app.panel = panel;
        for width in [1, 20, 60, 100, 140, 190] {
            for height in [1, 5, 16, 40] {
                render(&app, width, height);
            }
        }
    }
    press(&mut app, KeyCode::Char('n'));
    app.paste("What is the weather in LA?\n".repeat(20).as_str());
    for width in [1, 30, 80] {
        render(&app, width, 16);
    }
}

#[test]
fn old_answers_keep_their_own_sources_and_failure_is_visible_without_activity() {
    let mut app = Workspace::demo();
    let q = app.detail.as_mut().unwrap().question.as_mut().unwrap();
    let mut next = q.turns[0].clone();
    next.question = "Tomorrow?".into();
    next.state = State::Stopped;
    next.answer = None;
    next.error = Some("Weather provider timed out. Retry to continue.".into());
    q.turns.push(next);
    press(&mut app, KeyCode::Char(']'));
    let text = render(&app, 100, 32);
    assert!(text.contains("Weather provider timed out"));
    assert!(text.contains("r Retry"));
    press(&mut app, KeyCode::Char('s'));
    assert!(render(&app, 75, 32).contains("No sources cited"));
    press(&mut app, KeyCode::Char('['));
    assert!(render(&app, 75, 32).contains("Example weather provider"));
}

#[derive(Default)]
struct Source {
    calls: Mutex<Vec<(Option<String>, String)>>,
    opened: Mutex<Vec<String>>,
    agents: Mutex<Vec<String>>,
    detail: Mutex<Option<TaskDetail>>,
    replies: Mutex<Vec<conductor_model::interaction::InputResponse>>,
}
impl RunSource for Source {
    fn receipts(&self) -> Vec<conductor_model::Receipt> {
        vec![]
    }
    fn running(&self) -> Vec<conductor_model::view::LiveRun> {
        vec![]
    }
    fn live(&self, _: &str) -> Option<conductor_model::view::LiveRun> {
        None
    }
}
impl TaskSource for Source {
    fn has_agent(&self, _: &str) -> bool {
        true
    }
    fn open_agent(&self, id: &str) -> Result<(), String> {
        self.agents.lock().unwrap().push(id.into());
        Ok(())
    }

    fn tasks(&self) -> Vec<TaskSummary> {
        vec![
            self.detail
                .lock()
                .unwrap()
                .as_ref()
                .map(|d| d.summary.clone())
                .unwrap_or(workspace::demo_task().summary),
        ]
    }
    fn task(&self, _: &str) -> Option<TaskDetail> {
        self.detail
            .lock()
            .unwrap()
            .clone()
            .or_else(|| Some(workspace::demo_task()))
    }
    fn question(&self, parent: Option<&str>, text: &str) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((parent.map(str::to_owned), text.into()));
        Ok("q-demo".into())
    }
    fn respond(
        &self,
        response: &conductor_model::interaction::InputResponse,
    ) -> Result<String, String> {
        let mut guard = self.detail.lock().unwrap();
        let d = guard.as_mut().unwrap();
        let q = d.question.as_mut().unwrap();
        let text = q.resolve_input(response)?;
        let mut turn = q.turns.last().unwrap().clone();
        turn.question = text;
        turn.answer = None;
        turn.state = State::Working;
        turn.input_response = Some(response.clone());
        q.turns.push(turn);
        d.summary.state = State::Working;
        self.replies.lock().unwrap().push(response.clone());
        Ok(q.id.clone())
    }
    fn cancel_question(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn open_source(&self, url: &str) -> Result<(), String> {
        self.opened.lock().unwrap().push(url.into());
        Ok(())
    }
}

#[test]
fn follow_up_uses_the_active_task_and_a_new_question_does_not() {
    let src = Arc::new(Source::default());
    let mut app = Workspace::new(src.clone());
    app.open("q-demo");
    press(&mut app, KeyCode::Char('f'));
    app.paste("Tomorrow?");
    press(&mut app, KeyCode::Enter);
    for _ in 0..100 {
        app.tick();
        if src.calls.lock().unwrap().len() == 1 && app.input.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        src.calls.lock().unwrap()[0],
        (Some("q-demo".into()), "Tomorrow?".into())
    );
    press(&mut app, KeyCode::Char('n'));
    app.paste("A different question");
    press(&mut app, KeyCode::Enter);
    for _ in 0..100 {
        app.tick();
        if src.calls.lock().unwrap().len() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        src.calls.lock().unwrap()[1],
        (None, "A different question".into())
    );
}

#[test]
fn comparison_keeps_selection_through_sources_text_view_and_resize() {
    let mut app = Workspace::demo();
    app.open("q-comparison");
    let wide = render(&app, 140, 32);
    assert!(wide.contains("Los Angeles") && wide.contains("San Francisco"));
    press(&mut app, KeyCode::Down);
    assert_eq!(app.result_row, 1);
    let narrow = render(&app, 48, 32);
    assert!(narrow.contains("San Francisco") && narrow.contains("16°C / 61°F"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.panel, Some(Panel::Sources));
    assert_eq!(app.source_filter, Some(vec![0]));
    assert!(render(&app, 75, 32).contains("San Francisco"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('v'));
    assert!(app.text_view);
    assert!(render(&app, 100, 32).contains("City: San Francisco"));
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.result_row, 1);
    assert!(render(&app, 48, 32).contains("San Francisco"));
    app.open("q-forecast");
    assert_eq!(app.result_row, 0);
    press(&mut app, KeyCode::Down);
    assert!(render(&app, 48, 32).contains("Afternoon"));
}

#[test]
fn rows_without_citations_do_not_borrow_another_rows_evidence() {
    use conductor_model::task::{Fact, Presentation};
    let mut app = Workspace::demo();
    let turn = &mut app
        .detail
        .as_mut()
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .turns[0];
    let answer = turn.answer.take().unwrap();
    turn.answer = Some(answer.with_presentation(Presentation::Facts {
        title: "Weather".into(),
        summary: "Demo".into(),
        facts: vec![Fact {
            label: "Opinion".into(),
            value: "A pleasant day".into(),
            sources: vec![],
        }],
    }));
    press(&mut app, KeyCode::Enter);
    let panel = render(&app, 75, 32);
    assert!(panel.contains("No source attached"));
    assert!(!panel.contains("Captured tool response"));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.panel, Some(Panel::Sources));
    assert!(render(&app, 75, 32).contains("Example weather provider"));
}

#[test]
fn tests_link_to_captured_failure_and_keep_run_status_separate() {
    let mut app = Workspace::demo();
    app.open("test-demo");
    let text = render(&app, 140, 32);
    assert!(text.contains("1 failed · 2 passed · 0 ignored") && text.contains("Stopped"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.panel, Some(Panel::Evidence));
    assert!(render(&app, 75, 32).contains("actual status: 404"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(render(&app, 75, 32).contains("uppercase_account_id"));
    assert!(!render(&app, 75, 32).contains("actual status: 404"));
}

#[test]
fn every_result_view_renders_at_tiny_sizes() {
    let mut app = Workspace::demo();
    for id in [
        "q-demo",
        "q-comparison",
        "q-forecast",
        "test-demo",
        "bug-demo",
    ] {
        app.open(id);
        for width in [1, 20, 48, 80, 140] {
            for height in [1, 5, 16, 32] {
                render(&app, width, height);
                if id == "bug-demo" {
                    app.change_focus = true;
                    render(&app, width, height);
                    app.change_focus = false;
                }
            }
        }
    }
}

#[test]
fn filtered_sources_open_the_visible_url_and_an_empty_selection_opens_nothing() {
    use conductor_model::task::{Citation, Fact, Presentation};
    let source = Arc::new(Source::default());
    let mut app = Workspace::new(source.clone());
    app.open("q-demo");
    let turn = &mut app
        .detail
        .as_mut()
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .turns[0];
    let mut answer = turn.answer.take().unwrap();
    answer.sources.push(Citation {
        title: "Second provider".into(),
        url: "https://example.com/second".into(),
        ..Citation::default()
    });
    turn.answer = Some(answer.with_presentation(Presentation::Facts {
        title: "Weather".into(),
        summary: "Demo".into(),
        facts: vec![
            Fact {
                label: "Compared".into(),
                value: "Both".into(),
                sources: vec![2, 1],
            },
            Fact {
                label: "Uncited".into(),
                value: "Unknown".into(),
                sources: vec![],
            },
        ],
    }));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.source_filter, Some(vec![0, 1]));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(
        source.opened.lock().unwrap().as_slice(),
        &["https://example.com/second"]
    );
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(source.opened.lock().unwrap().len(), 1);
}

#[test]
fn invalid_table_is_readable_as_text_without_turning_into_a_report() {
    use conductor_model::task::{Presentation, ResultRow};
    let mut app = Workspace::demo();
    let turn = &mut app
        .detail
        .as_mut()
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .turns[0];
    let answer = turn.answer.take().unwrap();
    turn.answer = Some(answer.with_presentation(Presentation::Table {
        title: "Broken".into(),
        summary: "Demo".into(),
        columns: vec!["City".into(), "Temp".into()],
        rows: vec![ResultRow {
            cells: vec!["LA".into()],
            sources: vec![10],
        }],
    }));
    let output = render(&app, 100, 32);
    assert!(output.contains("Invalid result data"));
    assert!(output.contains("24°C / 75°F"));
    assert!(!output.contains("Test report"));
}

#[test]
fn focused_task_uses_the_whole_pane_and_the_drawer_returns_without_losing_selection() {
    let mut app = Workspace::demo();
    app.focused = true;
    app.open("q-comparison");
    press(&mut app, KeyCode::Down);
    let selected = app.result_row;
    for width in [80, 120, 180] {
        let text = render(&app, width, 24);
        assert!(!text.contains("New question"));
        assert!(!text.contains("Cargo test report"));
        assert!(text.contains("f Follow up"));
        assert!(text.contains("s Sources"));
    }
    press(&mut app, KeyCode::Tab);
    assert!(app.list_focus);
    press(&mut app, KeyCode::Esc);
    assert!(!app.list_focus);
    assert_eq!(app.result_row, selected);
    press(&mut app, KeyCode::Char('s'));
    render(&app, 80, 24);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.result_row, selected);
}

#[test]
fn visiting_an_agent_preserves_the_result_selection_and_inspector() {
    let src = Arc::new(Source::default());
    let mut app = Workspace::new(src.clone());
    app.open("q-demo");
    app.tick();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('s'));
    let row = app.result_row;
    let panel = app.panel;
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(*src.agents.lock().unwrap(), ["q-demo"]);
    assert_eq!(app.result_row, row);
    assert_eq!(app.panel, panel);
    assert_eq!(app.detail.as_ref().unwrap().summary.id, "q-demo");
    assert!(!app.quit);
}

#[test]
fn failed_refresh_retains_only_the_answer_to_the_same_question_and_its_sources() {
    let source = Arc::new(Source::default());
    let mut app = Workspace::new(source.clone());
    app.open("q-demo");
    let q = app.detail.as_mut().unwrap().question.as_mut().unwrap();
    let mut retry = q.turns[0].clone();
    retry.state = State::Stopped;
    retry.answer = None;
    retry.error = Some("Provider unavailable".into());
    q.turns.push(retry);
    press(&mut app, KeyCode::Char(']'));
    let output = render(&app, 120, 32);
    assert!(output.contains("Provider unavailable"));
    assert!(output.contains("Previous answer"));
    assert!(output.contains("24°C / 75°F"));
    press(&mut app, KeyCode::Char('1'));
    assert!(render(&app, 75, 32).contains("Captured tool response"));
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(
        source.opened.lock().unwrap().as_slice(),
        &["https://example.com/weather"]
    );
}

#[test]
fn patches_and_checks_share_an_attempt_and_open_evidence_does_not_change_under_the_reader() {
    use conductor_model::task::TaskChange;
    let mut app = Workspace::demo();
    app.focused = true;
    app.open("test-demo");
    app.detail.as_mut().unwrap().changes = vec![TaskChange {
        stage: "test".into(),
        attempt: 1,
        path: "src/lookup.rs".into(),
        patch: "diff --git a/src/lookup.rs b/src/lookup.rs\n-old_lookup\n+normalized_lookup\n"
            .into(),
    }];
    let wide = render(&app, 140, 36);
    assert!(wide.contains("+normalized_lookup"));
    assert!(wide.contains("lowercase_account_id"));
    let narrow = render(&app, 80, 24);
    assert!(narrow.contains("lowercase_account_id"));
    press(&mut app, KeyCode::Char('p'));
    assert!(render(&app, 80, 24).contains("+normalized_lookup"));
    press(&mut app, KeyCode::Char('p'));
    press(&mut app, KeyCode::Enter);
    app.detail.as_mut().unwrap().evidence[0].1 = "Different attempt passed".into();
    assert!(render(&app, 75, 32).contains("actual status: 404"));
    assert!(!render(&app, 75, 32).contains("Different attempt passed"));
    press(&mut app, KeyCode::Esc);
    app.detail.as_mut().unwrap().changes[0].attempt = 2;
    let unmatched = render(&app, 140, 32);
    assert!(unmatched.contains("No test cases recorded"));
    assert!(!unmatched.contains("uppercase_account_id"));
}

#[test]
fn compact_question_keeps_follow_up_close_and_activity_hides_transport_metadata() {
    let mut app = Workspace::demo();
    app.focused = true;
    let output = render(&app, 140, 60);
    let follow = output
        .lines()
        .position(|line| line.contains("f Follow up"))
        .unwrap();
    assert!(follow < 30);
    assert!(!output.contains("Fact             Value"));
    press(&mut app, KeyCode::Char('a'));
    let output = render(&app, 75, 32);
    assert!(!output.contains("2026-09-26T"));
    press(&mut app, KeyCode::Enter);
    assert!(render(&app, 75, 32).contains("2026-09-26T"));
}

#[test]
fn a_passing_case_count_cannot_hide_a_flaky_gate_in_a_patch_view() {
    let mut app = Workspace::demo();
    app.open("bug-demo");
    app.detail.as_mut().unwrap().tests.as_mut().unwrap().notices = vec![
        "fix · attempt 2 · check 1 (command_assert) · gate flaky".into(),
        "fix · attempt 2 · check 1 (command_assert) · Cases show the latest parsed report; the gate verdict includes all reruns.".into(),
    ];
    for width in [80, 120, 140] {
        let output = render(&app, width, 32);
        assert!(output.contains("4 passed"));
        assert!(output.contains("Gate flaky"));
        assert!(output.contains("reruns"));
    }
}

struct NativeSource {
    operations: Mutex<Vec<conductor_model::capability::Available>>,
    calls: Mutex<usize>,
}
impl RunSource for NativeSource {
    fn receipts(&self) -> Vec<conductor_model::Receipt> {
        vec![]
    }
    fn running(&self) -> Vec<conductor_model::view::LiveRun> {
        vec![]
    }
    fn live(&self, _: &str) -> Option<conductor_model::view::LiveRun> {
        None
    }
}
impl TaskSource for NativeSource {
    fn capabilities(&self) -> Result<Vec<conductor_model::capability::Available>, String> {
        Ok(self.operations.lock().unwrap().clone())
    }
    fn invoke(
        &self,
        _: &conductor_model::capability::Binding,
        _: Option<&str>,
    ) -> Result<String, String> {
        *self.calls.lock().unwrap() += 1;
        Ok("n-native".into())
    }
    fn tasks(&self) -> Vec<TaskSummary> {
        vec![self.task("n-native").unwrap().summary]
    }
    fn task(&self, _: &str) -> Option<TaskDetail> {
        use conductor_model::capability::{Capture, NativeView};
        let mut d = workspace::demo_task();
        d.summary = TaskSummary {
            id: "n-native".into(),
            title: "Imported report".into(),
            kind: "Native operation".into(),
            state: State::Finished,
        };
        d.question = None;
        d.context = "local report · 0 agent calls".into();
        d.evidence = vec![("Captured result".into(), "Exact report snapshot".into())];
        d.native = Some(NativeView {
            media_type: "application/json".into(),
            answer: Some(
                Capture::new(
                    "fixture".into(),
                    "application/json".into(),
                    r#"[{"issue":"APP-142","status":"failed"}]"#.into(),
                )
                .unwrap()
                .answer("Imported report"),
            ),
            refresh: self
                .operations
                .lock()
                .unwrap()
                .first()
                .map(|o| o.binding.clone()),
            notice: None,
        });
        Some(d)
    }
    fn question(&self, _: Option<&str>, _: &str) -> Result<String, String> {
        panic!("Native rendering must not start an agent")
    }
    fn cancel_question(&self, _: &str) -> Result<(), String> {
        panic!("Not an agent task")
    }
}

#[test]
fn dynamic_actions_reuse_results_and_preserve_the_composer_and_evidence() {
    use conductor_model::capability::{Available, Binding};
    let operation = Available {
        binding: Binding {
            id: "custom.report".into(),
            fingerprint: "v1".into(),
        },
        label: "Custom report".into(),
        detail: "Read a file".into(),
        unavailable: None,
    };
    let src = Arc::new(NativeSource {
        operations: Mutex::new(vec![operation.clone()]),
        calls: Mutex::new(0),
    });
    let mut app = Workspace::new(src.clone());
    app.paste("Keep this question draft");
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert!(render(&app, 80, 24).contains("Custom report"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.text, "Keep this question draft");
    assert_eq!(app.input, Some(Input::Question));
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let mut second = operation;
    second.binding.id = "ops.report".into();
    second.label = "Ops report".into();
    src.operations.lock().unwrap().push(second);
    press(&mut app, KeyCode::Char('r'));
    assert!(render(&app, 80, 24).contains("Ops report"));
    assert_eq!(*src.calls.lock().unwrap(), 0);
    press(&mut app, KeyCode::Enter);
    for _ in 0..100 {
        app.tick();
        if app.detail.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.detail.is_some());
    assert_eq!(*src.calls.lock().unwrap(), 1);
    assert!(render(&app, 100, 28).contains("APP-142"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.panel, Some(Panel::Evidence));
    assert!(render(&app, 75, 24).contains("Exact report snapshot"));
    for width in [1, 40, 80, 140] {
        render(&app, width, 24);
    }
    press(&mut app, KeyCode::Esc);
    app.tick();
    assert_eq!(*src.calls.lock().unwrap(), 1);
    press(&mut app, KeyCode::Char('r'));
    for _ in 0..100 {
        app.tick();
        if *src.calls.lock().unwrap() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(*src.calls.lock().unwrap(), 2);
}

#[test]
fn prose_uses_a_markdown_document_and_source_view_preserves_the_original() {
    use conductor_model::task::{Answer, Citation};
    let mut app = Workspace::demo();
    let md = "## Where to apply\n\n**Company:** Read the *role* and use `apply`.\n\n- Prepare a concrete example of your work and a clear account of your contribution.\n- [Careers](https://example.com/jobs) [1]\n\n```sh\necho '**literal**'\n```";
    app.detail
        .as_mut()
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .turns[0]
        .answer = Some(Answer {
        text: md.into(),
        sources: vec![Citation {
            title: "Careers".into(),
            url: "https://example.com/jobs".into(),
            ..Citation::default()
        }],
        ..Answer::default()
    });
    for width in [60, 100, 140] {
        let text = render(&app, width, 40);
        assert!(text.contains("Answer · Markdown"));
        assert!(text.contains("Where to apply"));
        assert!(!text.contains("**Company:**"));
        assert!(!text.contains("*role*"));
        assert!(!text.contains("](https://"));
        assert!(!text.contains("```sh"));
        assert!(text.contains("**literal**"));
        assert!(text.contains("1 source · s inspect"));
    }
    press(&mut app, KeyCode::Char('v'));
    let source = render(&app, 100, 40);
    assert!(source.contains("Markdown source"));
    assert!(source.contains("**Company:**"));
    assert_eq!(app.current_answer().unwrap().text, md);
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('s'));
    assert!(render(&app, 75, 40).contains("Careers"));
}

#[test]
fn facts_are_bounded_and_long_context_remains_available() {
    use conductor_model::task::{Fact, Presentation};
    let mut app = Workspace::demo();
    let answer = app
        .detail
        .as_mut()
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .turns[0]
        .answer
        .as_mut()
        .unwrap();
    answer.presentation=serde_json::to_value(Presentation::Facts{title:"Melbourne observation".into(),summary:format!("Observation is from earlier today. {}Never substitute retrieval time for observation time.","Keep the observation caveat. ".repeat(15)),facts:vec![Fact{label:"Conditions".into(),value:"Sunny".into(),sources:vec![1]},Fact{label:"Temperature".into(),value:"14°C / 56°F".into(),sources:vec![1]},Fact{label:"Wind".into(),value:"20 km/h from the south".into(),sources:vec![1]}]}).ok();
    let wide = render(&app, 140, 40);
    assert!(wide.contains("Facts"));
    assert!(wide.contains("Sunny"));
    assert!(wide.contains("14°C / 56°F"));
    assert!(
        wide.lines()
            .any(|l| l.contains("Conditions") && l.contains("Temperature"))
    );
    assert!(wide.contains("v full text for all context"));
    assert!(!wide.lines().next().unwrap_or("").is_empty());
    press(&mut app, KeyCode::Char('v'));
    app.scroll = 200;
    let full = render(&app, 70, 45);
    assert!(full.contains("observation time"));
}

#[test]
fn native_markdown_is_rendered_but_plain_text_and_json_remain_literal() {
    use conductor_model::capability::NativeView;
    use conductor_model::task::Answer;
    for media in ["text/markdown", "text/plain", "application/json"] {
        let mut app = Workspace::demo();
        let d = app.detail.as_mut().unwrap();
        d.question = None;
        d.native = Some(NativeView {
            media_type: media.into(),
            answer: Some(Answer {
                text: "# Release notes\n\n**Keep this text**".into(),
                ..Answer::default()
            }),
            refresh: None,
            notice: None,
        });
        let out = render(&app, 80, 24);
        if media == "text/markdown" {
            assert!(out.contains("Document · Markdown"));
            assert!(!out.contains("**Keep this text**"));
        } else {
            assert!(out.contains("**Keep this text**"));
        }
    }
}

fn clarification_app() -> (Arc<Source>, Workspace) {
    let src = Arc::new(Source::default());
    *src.detail.lock().unwrap() = Some(workspace::demo_clarification());
    let mut app = Workspace::new(src.clone());
    assert!(app.open("q-clarification"));
    (src, app)
}
fn await_reply(app: &mut Workspace) {
    for _ in 0..100 {
        app.tick();
        if app
            .current_turn()
            .is_some_and(|t| t.input_response.is_some())
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("response not received: {:?}", app.status);
}
#[test]
fn clarification_selection_retains_context_and_submits_once_to_the_same_task() {
    let (src, mut app) = clarification_app();
    let out = render(&app, 98, 38);
    assert!(out.contains("Clarify") && out.contains("My own answer"));
    assert!(!out.contains("No sources cited") && !out.contains("Answer · Markdown"));
    press(&mut app, KeyCode::Enter);
    assert!(src.replies.lock().unwrap().is_empty());
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('c'));
    app.paste("Security certification");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    render(&app, 48, 24);
    render(&app, 140, 40);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    await_reply(&mut app);
    let replies = src.replies.lock().unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].option.as_deref(), Some("encryption"));
    assert_eq!(replies[0].context, "Security certification");
    assert_eq!(replies[0].binding.task, "q-clarification");
    assert_eq!(replies[0].binding.turn, 1);
    drop(replies);
    assert!(render(&app, 98, 38).contains("Full Disk Encryption"));
    press(&mut app, KeyCode::Char('['));
    assert!(render(&app, 98, 38).contains("Recorded: Full Disk Encryption"));
    press(&mut app, KeyCode::Enter);
    assert!(app.text_view);
    assert!(render(&app, 98, 38).contains("Recorded response"));
    assert_eq!(src.replies.lock().unwrap().len(), 1);
}
#[test]
fn custom_clarification_handles_unicode_and_rejects_an_expired_draft() {
    let (src, mut app) = clarification_app();
    press(&mut app, KeyCode::Char('f'));
    app.paste("Météo qrs\nSomething different");
    assert_eq!(app.input, Some(Input::ClarificationAnswer));
    assert!(!app.quit);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(app.text, "Météo qrs\nSomething different");
    // Another pane advances the task while this draft is being edited.
    let mut d = src.detail.lock().unwrap();
    let q = d.as_mut().unwrap().question.as_mut().unwrap();
    let mut next = q.turns[0].clone();
    next.question = "A newer request".into();
    q.turns.push(next);
    drop(d);
    app.tick();
    press(&mut app, KeyCode::Enter);
    assert!(app.status.as_ref().unwrap().contains("no longer current"));
    assert!(src.replies.lock().unwrap().is_empty());
    assert_eq!(app.text, "Météo qrs\nSomething different");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Char('f'));
    app.paste("Field development engineering");
    press(&mut app, KeyCode::Enter);
    await_reply(&mut app);
    let replies = src.replies.lock().unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].binding.turn, 2);
    assert_eq!(replies[0].option, None);
    assert_eq!(replies[0].text, "Field development engineering");
}
#[test]
fn unsupported_requests_keep_prose_and_clarification_renders_in_small_panes() {
    let (_, mut app) = clarification_app();
    for width in [1, 20, 48, 80, 98, 140] {
        for height in [1, 5, 16, 24, 40] {
            render(&app, width, height);
        }
    }
    press(&mut app, KeyCode::Char('f'));
    app.paste("custom answer");
    for width in [1, 20, 48, 98] {
        render(&app, width, 16);
    }
    press(&mut app, KeyCode::Esc);
    app.detail
        .as_mut()
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .turns[0]
        .answer
        .as_mut()
        .unwrap()
        .input_request = Some(serde_json::json!({"kind":"execute","command":"anything"}));
    let out = render(&app, 98, 40);
    assert!(out.contains("Unsupported clarification"));
    assert!(out.contains("What does FDE mean"));
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(app.input, Some(Input::FollowUp));
}
