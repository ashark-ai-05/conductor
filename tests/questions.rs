//! Full background-question lifecycle, without a model or network call.
use conductor_engine::question;
use conductor_model::task::{Question, State};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FAKE: &str = r#"#!/usr/bin/env python3
import json, sys, time, os
args=sys.argv
assert '--tools' in args and args[args.index('--tools')+1]=='WebSearch,WebFetch'
assert '--safe-mode' in args and '--strict-mcp-config' in args
assert not os.path.exists('.git')
prompt=args[args.index('-p')+1]
assert 'Current UTC time:' in prompt
schema=json.loads(args[args.index('--json-schema')+1])
assert 'presentation' in schema['properties']
assert 'input_request' in schema['properties']
assert len(schema['properties']['presentation']['anyOf']) == 3
current=json.loads(prompt.split('Prior conversation follows as JSON data; the last question is the current request:\n')[1])[-1]['question']
def emit(v): print(json.dumps(v),flush=True)
emit({'type':'system','subtype':'init','session_id':'test-session'})
emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'t1','name':'WebFetch','input':{'url':'https://example.com/weather','prompt':'Current conditions'}}]}})
if 'slow question' in prompt: time.sleep(10)
emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'t1','content':'24 C, clear; observed 2026-09-26T17:00:00Z'}]}})
if 'fail question' in prompt:
    emit({'type':'result','subtype':'error_during_execution','is_error':True,'result':''})
    sys.exit(1)
if 'Tomorrow?' in prompt: assert 'Los Angeles' in prompt and '24' in prompt
text='Los Angeles: 24 C, clear. Observed 10:00 PDT [1].'
answer={'text':text,'needs_input':False,'sources':[{'title':'Example weather','url':'https://example.com/weather','supports':'Temperature and conditions','observed_at':'2026-09-26T17:00:00Z','retrieved_at':'invented','captured':'invented'}]}
answer['input_request']=None
answer['presentation']={'kind':'facts','title':'Los Angeles','summary':'Observed 10:00 PDT; fixture data','facts':[{'label':'Temperature','value':'24 C','sources':[1]}]}
if current.startswith('Compare'):
    answer['text']='LA: 24 C; SF: 16 C. Fixture comparison [1].'
    answer['presentation']={'kind':'table','title':'Cities','summary':'Fixture comparison','columns':['City','Temperature'],'rows':[{'cells':['LA','24 C'],'sources':[1]},{'cells':['SF','16 C'],'sources':[1]}]}
elif current == 'Tomorrow?':
    answer['text']='Los Angeles tomorrow: 27 C. Fixture forecast [1].'
    answer['presentation']={'kind':'table','title':'Forecast','summary':'Tomorrow; fixture forecast','columns':['Period','Temperature'],'rows':[{'cells':['Afternoon','27 C'],'sources':[1]}]}
if current == 'clarify FDE':
    answer={'text':'What does FDE mean? Forward Deployed Engineer or Full Disk Encryption?','needs_input':False,'sources':[],'presentation':None,'input_request':{'kind':'single_choice','question':'What does FDE mean?','explanation':'Choose the subject.','options':[{'id':'fde','label':'Forward Deployed Engineer','description':'Engineering role'},{'id':'disk','label':'Full Disk Encryption','description':'Security'}]}}
elif current.startswith('Clarification:'):
    history=json.loads(prompt.split('Prior conversation follows as JSON data; the last question is the current request:\n')[1])
    assert history[-1]['input_response']['binding']['turn']==len(history)-1
    assert history[-2]['answer']['input_request']['kind']=='single_choice'
    answer={'text':'Resources for your selected subject. '+current,'needs_input':False,'sources':[],'presentation':None,'input_request':None}
emit({'type':'result','subtype':'success','is_error':False,'result':'','structured_output':answer})
"#;

fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let fake = d.path().join("fake-claude");
    executable(&fake, FAKE);
    let bin = d.path().join("conductor-test");
    // A wrapper supplies the per-test executor without mutating the test process's environment.
    executable(
        &bin,
        &format!(
            "#!/usr/bin/env python3\nimport os,sys\nos.environ['CONDUCTOR_CLAUDE_BIN']={}\nos.execv({},[{},*sys.argv[1:]])\n",
            serde_json::to_string(&fake.to_str().unwrap()).unwrap(),
            serde_json::to_string(env!("CARGO_BIN_EXE_conductor")).unwrap(),
            serde_json::to_string(env!("CARGO_BIN_EXE_conductor")).unwrap()
        ),
    );
    (d, bin)
}

fn wait(repo: &Path, id: &str) -> Question {
    let start = Instant::now();
    loop {
        let q = question::read(repo, id).unwrap();
        if q.turns.last().unwrap().state != State::Working {
            // The worker releases its reservation immediately after publishing completion.
            while repo
                .join(".conductor/questions")
                .join(id)
                .join("busy")
                .exists()
            {
                assert!(start.elapsed() < Duration::from_secs(15));
                std::thread::sleep(Duration::from_millis(10));
            }
            return q;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "{q:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_question_and_follow_up_need_no_workflow_git_or_approval_and_preserve_evidence() {
    let (d, bin) = setup();
    let repo = d.path();
    let id = question::launch(repo, &bin, None, "What is the weather in LA?").unwrap();
    let first = wait(repo, &id);
    let turn = &first.turns[0];
    assert_eq!(turn.state, State::Answered, "{turn:?}");
    assert!(turn.tokens.is_none() && turn.cost_usd.is_none());
    let source = &turn.answer.as_ref().unwrap().sources[0];
    assert!(source.captured.as_ref().unwrap().contains("24 C"));
    assert_ne!(source.retrieved_at.as_deref(), Some("invented"));
    assert_eq!(source.observed_at.as_deref(), Some("2026-09-26T17:00:00Z"));
    assert!(!repo.join(".git").exists());
    assert!(!repo.join(".conductor/runs").exists());
    assert_eq!(
        question::launch(repo, &bin, Some(&id), "Tomorrow?").unwrap(),
        id
    );
    let next = wait(repo, &id);
    assert_eq!(next.turns.len(), 2);
    assert_eq!(next.turns[0], first.turns[0]);
    assert_eq!(next.turns[1].state, State::Answered, "{:?}", next.turns[1]);
}

#[test]
fn duplicate_submissions_are_refused_and_cancellation_preserves_the_question() {
    let (d, bin) = setup();
    let repo = d.path();
    let id = question::launch(repo, &bin, None, "slow question").unwrap();
    assert!(question::launch(repo, &bin, Some(&id), "second").is_err());
    question::cancel(repo, &id).unwrap();
    let q = wait(repo, &id);
    assert_eq!(q.turns[0].state, State::Cancelled);
    assert_eq!(q.turns[0].question, "slow question");
    assert_eq!(
        question::launch(repo, &bin, Some(&id), "Retry now").unwrap(),
        id
    );
    // Prior history still contains "slow question", so cancel the second attempt as well.
    question::cancel(repo, &id).unwrap();
    assert_eq!(wait(repo, &id).turns.len(), 2);
}

#[test]
fn a_failed_agent_is_stopped_with_a_reason_and_remains_in_the_task_list() {
    let (d, bin) = setup();
    let id = question::launch(d.path(), &bin, None, "fail question").unwrap();
    let q = wait(d.path(), &id);
    assert_eq!(q.turns[0].state, State::Stopped);
    assert!(
        q.turns[0]
            .error
            .as_ref()
            .unwrap()
            .contains("error_during_execution")
    );
    assert_eq!(question::list(d.path()).len(), 1);
}

#[test]
fn invalid_ids_and_missing_launch_programs_fail_without_a_stuck_reservation() {
    let (d, _) = setup();
    assert!(
        question::launch(
            d.path(),
            Path::new("/no-such-conductor"),
            Some("../../outside"),
            "x"
        )
        .is_err()
    );
    assert!(question::launch(d.path(), Path::new("/no-such-conductor"), None, "x").is_err());
    let qs = question::list(d.path());
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0].turns[0].state, State::Stopped);
    assert!(
        !d.path()
            .join(".conductor/questions")
            .join(&qs[0].id)
            .join("busy")
            .exists()
    );
}

#[test]
fn result_shapes_change_per_turn_and_old_views_remain_bound_to_their_sources() {
    use conductor_model::task::Presentation;
    let (d, bin) = setup();
    let id = question::launch(d.path(), &bin, None, "Weather in LA?").unwrap();
    let first = wait(d.path(), &id);
    assert!(matches!(
        first.turns[0]
            .answer
            .as_ref()
            .unwrap()
            .presentation()
            .unwrap(),
        Some(Presentation::Facts { .. })
    ));
    question::launch(d.path(), &bin, Some(&id), "Compare LA and SF").unwrap();
    let second = wait(d.path(), &id);
    assert_eq!(second.turns[0], first.turns[0]);
    assert!(
        matches!(second.turns[1].answer.as_ref().unwrap().presentation().unwrap(), Some(Presentation::Table { rows, .. }) if rows.len() == 2)
    );
    question::launch(d.path(), &bin, Some(&id), "Tomorrow?").unwrap();
    let third = wait(d.path(), &id);
    assert_eq!(third.turns[..2], second.turns[..]);
    assert!(
        matches!(third.turns[2].answer.as_ref().unwrap().presentation().unwrap(), Some(Presentation::Table { rows, .. }) if rows.len() == 1)
    );
}

#[test]
fn clarification_responses_resume_the_same_task_and_reject_stale_or_duplicate_input() {
    use conductor_model::interaction::InputResponse;
    let (d, bin) = setup();
    let id = question::launch(d.path(), &bin, None, "clarify FDE").unwrap();
    let first = wait(d.path(), &id);
    let (binding, _) = first
        .pending_input()
        .expect("a valid request overrides a missing needs_input flag");
    let response = InputResponse {
        binding,
        option: Some("fde".into()),
        text: String::new(),
        context: "Interview preparation".into(),
    };
    let mut invalid = response.clone();
    invalid.option = Some("invented".into());
    assert!(question::respond(d.path(), &bin, &invalid).is_err());
    assert_eq!(question::read(d.path(), &id).unwrap(), first);
    let mut stale = response.clone();
    stale.binding.fingerprint = "changed".into();
    assert!(question::respond(d.path(), &bin, &stale).is_err());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let response = &response;
                let bin = &bin;
                let repo = d.path();
                scope.spawn(move || {
                    barrier.wait();
                    question::respond(repo, bin, response)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    let next = wait(d.path(), &id);
    assert_eq!(next.turns.len(), 2);
    assert_eq!(next.turns[0], first.turns[0]);
    assert_eq!(next.turns[1].input_response.as_ref(), Some(&response));
    assert_eq!(next.turns[1].state, State::Answered, "{:?}", next.turns[1]);
    assert!(
        next.turns[1]
            .answer
            .as_ref()
            .unwrap()
            .text
            .contains("Interview preparation")
    );
    assert!(question::respond(d.path(), &bin, &response).is_err());
    question::launch(d.path(), &bin, Some(&id), "clarify FDE").unwrap();
    let again = wait(d.path(), &id);
    assert!(question::respond(d.path(), &bin, &response).is_err());
    let custom = InputResponse {
        binding: again.pending_input().unwrap().0,
        option: None,
        text: "Field development engineering".into(),
        context: String::new(),
    };
    question::respond(d.path(), &bin, &custom).unwrap();
    let final_q = wait(d.path(), &id);
    assert_eq!(final_q.turns.len(), 4);
    assert_eq!(final_q.turns[3].state, State::Answered);
    assert!(
        final_q.turns[3]
            .question
            .contains("Field development engineering")
    );
}

#[test]
fn a_recorded_clarification_survives_failed_worker_start_and_cannot_be_replayed() {
    use conductor_model::interaction::InputResponse;
    let (d, bin) = setup();
    let id = question::launch(d.path(), &bin, None, "clarify FDE").unwrap();
    let first = wait(d.path(), &id);
    let response = InputResponse {
        binding: first.pending_input().unwrap().0,
        option: Some("fde".into()),
        text: String::new(),
        context: String::new(),
    };
    assert!(question::respond(d.path(), Path::new("/no-such-conductor"), &response).is_err());
    let q = question::read(d.path(), &id).unwrap();
    assert_eq!(q.turns.len(), 2);
    assert_eq!(q.turns[1].input_response, Some(response.clone()));
    assert_eq!(q.turns[1].state, State::Stopped);
    assert!(question::respond(d.path(), &bin, &response).is_err());
}
