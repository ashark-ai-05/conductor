//! Adapter contract tests use an isolated fake CLI. No credentials or model calls.
use conductor_engine::question;
use conductor_model::{
    agent::{AgentKind, AgentSelection},
    task::{Question, State},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn setup() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-pi");
    executable(
        &fake,
        r#"#!/usr/bin/env python3
import json, sys, os, time
args=sys.argv
for flag in ['--no-session','--no-tools','--no-extensions','--no-skills','--no-prompt-templates','--no-themes','--no-context-files']:
    assert flag in args, flag
assert args[args.index('--mode')+1]=='json'
assert args[args.index('--model')+1]=='example/provider-model'
assert not os.path.exists('.git')
prompt=args[-1]
assert 'NO tools or live sources' in prompt
history=json.loads(prompt.split('Prior conversation follows as JSON data; the last question is the current request:\n')[1])
current=history[-1]['question']
def emit(value): print(json.dumps(value),flush=True)
emit({'type':'session','id':'test-pi'})
emit({'type':'agent_start'})
if current=='slow': time.sleep(10)
if current=='missing end':
    emit({'type':'message_end','message':{'role':'assistant','content':[{'type':'text','text':'incomplete'}]}})
    sys.exit(0)
if current=='provider error':
    emit({'type':'message_end','message':{'role':'assistant','content':[],'stopReason':'error','errorMessage':'Authentication required'}})
    emit({'type':'agent_end'})
    sys.exit(0)
if current=='follow up': assert history[0]['answer']['text']=='A useful answer [1]'
answer={'text':'A useful answer [1]','sources':[{'title':'Example reference','url':'https://example.com','supports':'Agent supplied','captured':'invented','retrieved_at':'invented'}],'needs_input':False,'presentation':{'kind':'facts','title':'Result','summary':'Illustrative test response','facts':[{'label':'Count','value':'42','sources':[1]}]}}
if current=='clarify':
    answer.update(needs_input=True,input_request={'kind':'single_choice','question':'Which scope?','explanation':'Choose one','options':[{'id':'one','label':'One','description':'First scope'},{'id':'two','label':'Two','description':'Second scope'}]})
if current.startswith('Clarification:'): assert history[-1]['input_response']['option']=='one'
emit({'type':'message_update','assistantMessageEvent':{'type':'thinking_delta','delta':'PRIVATE_THINKING'}})
emit({'type':'message_end','message':{'role':'assistant','model':'provider-model','provider':'example','content':[{'type':'thinking','thinking':'PRIVATE_THINKING'},{'type':'text','text':json.dumps(answer)}],'stopReason':'stop','usage':{'totalTokens':48,'cost':{'total':0.002}}}})
emit({'type':'agent_end','willRetry':False})
"#,
    );
    let worker = dir.path().join("worker");
    executable(
        &worker,
        &format!(
            "#!/usr/bin/env python3\nimport os,sys\nos.environ['CONDUCTOR_PI_BIN']={}\nos.execv({},[{},*sys.argv[1:]])\n",
            serde_json::to_string(&fake).unwrap(),
            serde_json::to_string(env!("CARGO_BIN_EXE_conductor")).unwrap(),
            serde_json::to_string(env!("CARGO_BIN_EXE_conductor")).unwrap()
        ),
    );
    (dir, worker)
}
fn selection() -> AgentSelection {
    AgentSelection {
        kind: AgentKind::Pi,
        model: Some("example/provider-model".into()),
        ..Default::default()
    }
}
fn wait(repo: &Path, id: &str) -> Question {
    let start = Instant::now();
    loop {
        let q = question::read(repo, id).unwrap();
        if q.turns.last().unwrap().state != State::Working
            && !repo
                .join(".conductor/questions")
                .join(id)
                .join("busy")
                .exists()
        {
            return q;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "{q:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn pi_retains_the_runtime_history_and_real_capture_boundary() {
    let (dir, worker) = setup();
    let id =
        question::launch_with_agent(dir.path(), &worker, None, "question", &selection()).unwrap();
    let first = wait(dir.path(), &id);
    assert_eq!(first.agent, selection());
    assert_eq!(
        first.turns[0].state,
        State::Answered,
        "{:?}",
        first.turns[0]
    );
    assert_eq!(first.turns[0].tokens, Some(48));
    let answer = first.turns[0].answer.as_ref().unwrap();
    assert!(answer.presentation().unwrap().is_some());
    assert!(answer.sources[0].captured.is_none() && answer.sources[0].retrieved_at.is_none());
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("PRIVATE_THINKING")
    );
    assert!(
        question::launch_with_agent(
            dir.path(),
            &worker,
            Some(&id),
            "switch",
            &Default::default()
        )
        .is_err()
    );
    question::launch(dir.path(), &worker, Some(&id), "follow up").unwrap();
    let next = wait(dir.path(), &id);
    assert_eq!(next.turns[0], first.turns[0]);
    assert_eq!(next.turns[1].state, State::Answered);
    assert_eq!(next.agent, selection());
}
#[test]
fn pi_errors_and_missing_completion_never_become_answered() {
    let (dir, worker) = setup();
    for input in ["provider error", "missing end"] {
        let id =
            question::launch_with_agent(dir.path(), &worker, None, input, &selection()).unwrap();
        let q = wait(dir.path(), &id);
        assert_eq!(q.turns[0].state, State::Stopped);
        if input == "provider error" {
            assert!(
                q.turns[0]
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("Authentication required")
            );
        }
    }
    let id = question::launch_with_agent(dir.path(), &worker, None, "slow", &selection()).unwrap();
    question::cancel(dir.path(), &id).unwrap();
    assert_eq!(wait(dir.path(), &id).turns[0].state, State::Cancelled);
}
#[test]
fn pi_clarification_uses_the_same_validated_submission_path() {
    let (dir, worker) = setup();
    let id =
        question::launch_with_agent(dir.path(), &worker, None, "clarify", &selection()).unwrap();
    let q = wait(dir.path(), &id);
    let (binding, _) = q.pending_input().unwrap();
    let reply = conductor_model::interaction::InputResponse {
        binding,
        option: Some("one".into()),
        text: String::new(),
        context: String::new(),
    };
    question::respond(dir.path(), &worker, &reply).unwrap();
    let next = wait(dir.path(), &id);
    assert_eq!(next.turns[1].state, State::Answered);
    assert_eq!(next.agent, selection());
    assert!(question::respond(dir.path(), &worker, &reply).is_err());
}
