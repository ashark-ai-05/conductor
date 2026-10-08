//! CLI adapter contract tests. Each worker gets a fake CLI; no real agent is invoked.
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
fn selection(kind: AgentKind) -> AgentSelection {
    AgentSelection {
        kind,
        model: (kind == AgentKind::Copilot).then(|| "example-model".into()),
        mode: (kind == AgentKind::Amp).then(|| "example-mode".into()),
        tools: matches!(kind, AgentKind::Codex | AgentKind::Hermes),
    }
}
fn setup(kind: AgentKind) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-cli");
    executable(
        &fake,
        r#"#!/usr/bin/env python3
import json,os,sys,time
from pathlib import Path
args=sys.argv
kind=os.environ['CONDUCTOR_TEST_KIND']
assert not Path('.git').exists()
if kind in ['codex','hermes']:
    def emit(e): print(json.dumps(e),flush=True)
    if kind=='codex':
        assert args[1]=='exec' and '--json' in args and '--ephemeral' in args
        assert args[args.index('--sandbox')+1]=='read-only'
        assert 'approval_policy="never"' in args
        assert '--dangerously-bypass-approvals-and-sandbox' not in args
        prompt=args[-1]
        emit({'type':'thread.started','thread_id':'fixture'})
    else:
        assert args[1:5]==['chat','--oneshot','--format','stream-json']
        assert '--yolo' not in args and '--ignore-user-config' not in args
        prompt=Path(args[args.index('--query-file')+1]).read_text()
        emit({'type':'system','subtype':'init','model':'example-model','session_id':'fixture'})
    assert 'Use your tools to answer' in prompt
    history=json.loads(prompt.split('Prior conversation follows as JSON data; the last question is the current request:\n')[1])
    current=history[-1]['question']
    if current=='slow': time.sleep(10)
    if current in ['policy error','tool violation']:
        emit({'type':'turn.failed','error':{'message':'Runtime permission denied'}} if kind=='codex' else {'type':'result','exit_code':1,'error':'Runtime permission denied','text':''})
        sys.exit(1)
    if current=='follow up': assert history[0]['answer']['text']=='A useful answer [1]'
    if current.startswith('Clarification:'): assert history[-1]['input_response']['option']=='one'
    answer={'text':'A useful answer [1]','sources':[{'title':'File','url':'file:///repo/example','supports':'Agent citation','captured':'invented','retrieved_at':'invented'}],'needs_input':False,'presentation':{'kind':'facts','title':'Result','summary':'Fixture','facts':[{'label':'Count','value':'42','sources':[1]}]},'sections':[{'id':'summary','title':'Summary','text':'A useful answer [1]','sources':[1],'presentation':None}]}
    if current=='clarify': answer.update(needs_input=True,input_request={'kind':'single_choice','question':'Which scope?','explanation':'Choose one','options':[{'id':'one','label':'One','description':'First scope'},{'id':'two','label':'Two','description':'Second scope'}]})
    if kind=='codex':
        emit({'type':'item.completed','item':{'id':'private','type':'reasoning','text':'PRIVATE_THINKING'}})
        emit({'type':'item.started','item':{'id':'cmd','type':'command_execution','command':'cat example','status':'in_progress'}})
        emit({'type':'item.completed','item':{'id':'cmd','type':'command_execution','command':'cat example','status':'completed','exit_code':0,'aggregated_output':'REAL_TOOL_OUTPUT'}})
        emit({'type':'item.completed','item':{'id':'answer','type':'agent_message','text':json.dumps(answer)}})
        if current!='missing end': emit({'type':'turn.completed','usage':{'input_tokens':20,'cached_input_tokens':10,'output_tokens':5}})
    else:
        emit({'type':'text','text':'INTERMEDIATE_DELTA'})
        emit({'type':'tool_use','name':'read_file','input':{'path':'/repo/example'}})
        emit({'type':'tool_result','name':'read_file','output':'REAL_TOOL_OUTPUT','duration_ms':5,'is_error':False})
        if current!='missing end': emit({'type':'result','exit_code':0,'text':json.dumps(answer),'tokens':{'input':20,'output':5,'total':25}})
    sys.exit(2 if current=='bad exit' else 0)
if kind=='copilot':
    for flag in ['--output-format','--available-tools','--no-custom-instructions','--disable-builtin-mcps','--no-ask-user','--no-auto-update','--no-remote-export']:
        assert flag in args, flag
    assert args[args.index('--available-tools')+1].startswith('--')
    assert args[args.index('--output-format')+1]=='json'
    assert args[args.index('--model')+1]=='example-model'
    assert '--allow-all' not in args and '--allow-all-tools' not in args
    assert os.environ['COPILOT_ALLOW_ALL']=='false'
    assert os.environ['GITHUB_COPILOT_PROMPT_MODE_EXTENSIONS']=='false'
else:
    for flag in ['--stream-json','--no-ide','--no-notifications','--no-color','--execute']:
        assert flag in args,flag
    assert args[args.index('--visibility')+1]=='private'
    assert args[args.index('--mode')+1]=='example-mode'
    assert '--model' not in args
    settings=json.loads(Path('.amp/settings.json').read_text())
    assert '--dangerously-allow-all' not in args
    assert os.environ['AMP_SKIP_UPDATE_CHECK']=='1'
prompt=args[-1]
tools='Use your tools to answer' in prompt
if kind=='amp' and tools:
    assert 'amp.tools.disable' not in settings and 'amp.mcpPermissions' not in settings
    def emit(e): print(json.dumps(e),flush=True)
    emit({'type':'system','subtype':'init','tools':['Read','Bash','web_search'],'agent_mode':'example-mode'})
    emit({'type':'assistant','message':{'content':[{'type':'text','text':'Looking.'},{'type':'tool_use','id':'call1','name':'Read','input':{'path':'/repo/Jenkinsfile'}}],'stop_reason':'tool_use','usage':{'input_tokens':10,'output_tokens':2}}})
    emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'call1','content':'branch = env.DEFAULT_BRANCH','is_error':False}]}})
    answer={'text':'The Jenkinsfile reads DEFAULT_BRANCH [1]','needs_input':False,'sources':[{'title':'Jenkinsfile','url':'file:///repo/Jenkinsfile','supports':'Where the branch comes from','captured':'invented'}]}
    emit({'type':'assistant','message':{'content':[{'type':'text','text':json.dumps(answer)}],'stop_reason':'end_turn','usage':{'input_tokens':20,'output_tokens':5}}})
    emit({'type':'result','subtype':'success','is_error':False,'result':json.dumps(answer),'usage':{'input_tokens':30,'output_tokens':7}})
    sys.exit(0)
if kind=='amp':
    assert settings['amp.tools.disable']==['*']
    assert all(r['action']=='reject' for r in settings['amp.mcpPermissions'])
assert 'NO tools or live sources' in prompt
history=json.loads(prompt.split('Prior conversation follows as JSON data; the last question is the current request:\n')[1])
current=history[-1]['question']
def emit(e): print(json.dumps(e),flush=True)
if kind=='amp': emit({'type':'system','subtype':'init','tools':[],'agent_mode':'example-mode'})
else: emit({'type':'assistant.turn_start','data':{'turnId':'turn1'}})
if current=='slow': time.sleep(10)
if current=='policy error':
    if kind=='copilot': emit({'type':'session.error','data':{'message':'Access denied by policy settings','errorType':'authorization'}})
    else: emit({'type':'result','subtype':'error_during_execution','is_error':True,'error':'Authentication required'})
    sys.exit(1)
if current=='tool violation':
    if kind=='copilot': emit({'type':'tool.execution_start','data':{'toolName':'shell'}})
    else: emit({'type':'system','subtype':'init','tools':['Bash']})
    time.sleep(10)
    sys.exit(1)
if current=='follow up': assert history[0]['answer']['text']=='A useful answer [1]'
if current.startswith('Clarification:'): assert history[-1]['input_response']['option']=='one'
answer={'text':'A useful answer [1]','sources':[{'title':'Reference','url':'https://example.com','supports':'Agent supplied','captured':'invented','retrieved_at':'invented'}],'needs_input':False,'presentation':{'kind':'facts','title':'Result','summary':'Fixture','facts':[{'label':'Count','value':'42','sources':[1]}]}}
if current=='clarify': answer.update(needs_input=True,input_request={'kind':'single_choice','question':'Which scope?','explanation':'Choose one','options':[{'id':'one','label':'One','description':'First scope'},{'id':'two','label':'Two','description':'Second scope'}]})
if kind=='copilot':
    emit({'type':'assistant.reasoning_delta','data':{'deltaContent':'PRIVATE_THINKING'}})
    emit({'type':'assistant.message','agentId':'subagent','data':{'content':'WRONG_AGENT'}})
    emit({'type':'assistant.message','data':{'phase':'analysis','content':'PRIVATE_THINKING'}})
    emit({'type':'assistant.message','data':{'content':json.dumps(answer),'reasoningText':'PRIVATE_THINKING','model':'example-model'}})
    usage={'type':'assistant.usage','id':'one-usage','data':{'model':'example-model','inputTokens':20,'outputTokens':5,'cost':3,'finishReason':'stop','availableToolCount':0}}
    emit(usage);emit(usage)
    if current!='missing end': emit({'type':'assistant.turn_end','data':{'turnId':'turn1'}})
else:
    emit({'type':'assistant','message':{'content':[{'type':'thinking','thinking':'PRIVATE_THINKING'},{'type':'text','text':json.dumps(answer)}],'stop_reason':'end_turn','usage':{'input_tokens':20,'output_tokens':5}}})
    if current!='missing end': emit({'type':'result','subtype':'success','is_error':False,'result':json.dumps(answer),'usage':{'input_tokens':20,'output_tokens':5}})
if current=='bad exit': sys.exit(2)
"#,
    );
    let worker = dir.path().join("worker");
    let (name, env) = match kind {
        AgentKind::Copilot => ("copilot", "CONDUCTOR_COPILOT_BIN"),
        AgentKind::Amp => ("amp", "CONDUCTOR_AMP_BIN"),
        AgentKind::Codex => ("codex", "CONDUCTOR_CODEX_BIN"),
        AgentKind::Hermes => ("hermes", "CONDUCTOR_HERMES_BIN"),
        _ => unreachable!(),
    };
    let bin = serde_json::to_string(env!("CARGO_BIN_EXE_conductor")).unwrap();
    executable(
        &worker,
        &format!(
            "#!/usr/bin/env python3\nimport os,sys\nos.environ['CONDUCTOR_TEST_KIND']='{name}'\nos.environ['{env}']={}\nos.execv({bin},[{bin},*sys.argv[1:]])\n",
            serde_json::to_string(&fake).unwrap()
        ),
    );
    (dir, worker)
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
fn cli_adapters_keep_widgets_history_usage_and_provenance() {
    for kind in [
        AgentKind::Copilot,
        AgentKind::Amp,
        AgentKind::Codex,
        AgentKind::Hermes,
    ] {
        let (dir, worker) = setup(kind);
        let id =
            question::launch_with_agent(dir.path(), &worker, None, "question", &selection(kind))
                .unwrap();
        let first = wait(dir.path(), &id);
        assert_eq!(first.agent, selection(kind));
        assert_eq!(
            first.turns[0].state,
            State::Answered,
            "{:?}",
            first.turns[0]
        );
        assert_eq!(first.turns[0].tokens, Some(25));
        assert_eq!(first.turns[0].cost_usd, None);
        let answer = first.turns[0].answer.as_ref().unwrap();
        assert!(answer.presentation().unwrap().is_some());
        if matches!(kind, AgentKind::Codex | AgentKind::Hermes) {
            assert_eq!(answer.sections().unwrap().len(), 1);
            assert!(
                first.turns[0]
                    .activity
                    .iter()
                    .any(|a| a.detail.contains("REAL_TOOL_OUTPUT"))
            );
            assert!(
                !serde_json::to_string(&first)
                    .unwrap()
                    .contains("INTERMEDIATE_DELTA")
            );
        }
        assert!(answer.sources[0].captured.is_none() && answer.sources[0].retrieved_at.is_none());
        let saved = serde_json::to_string(&first).unwrap();
        assert!(!saved.contains("PRIVATE_THINKING") && !saved.contains("WRONG_AGENT"));
        question::launch(dir.path(), &worker, Some(&id), "follow up").unwrap();
        let next = wait(dir.path(), &id);
        assert_eq!(next.turns[0], first.turns[0]);
        assert_eq!(next.turns[1].state, State::Answered);
        assert_eq!(next.agent, first.agent);
    }
}
#[test]
fn amp_with_tools_records_each_call_and_what_it_read() {
    let (dir, worker) = setup(AgentKind::Amp);
    let mut with_tools = selection(AgentKind::Amp);
    with_tools.tools = true;
    let id =
        question::launch_with_agent(dir.path(), &worker, None, "question", &with_tools).unwrap();
    let q = wait(dir.path(), &id);
    let turn = &q.turns[0];
    assert_eq!(turn.state, State::Answered, "{turn:?}");
    assert!(q.agent.tools);
    let steps: Vec<_> = turn.activity.iter().map(|a| a.title.as_str()).collect();
    assert!(steps.contains(&"Reading a file"), "{steps:?}");
    assert!(steps.contains(&"Read returned"), "{steps:?}");
    let connected = turn
        .activity
        .iter()
        .find(|a| a.title == "Agent connected")
        .unwrap();
    assert!(connected.detail.contains("3 tools"));
    // The capture is what the tool returned, never what the answer claimed.
    let source = &turn.answer.as_ref().unwrap().sources[0];
    assert_eq!(
        source.captured.as_deref(),
        Some("branch = env.DEFAULT_BRANCH")
    );
    assert!(source.retrieved_at.is_some());
    // Tools stay off for an agent that has no switch.
    let mut copilot = selection(AgentKind::Copilot);
    copilot.tools = true;
    assert!(copilot.validate().is_err());
}
#[test]
fn errors_incomplete_streams_and_nonzero_exits_stay_stopped() {
    for kind in [
        AgentKind::Copilot,
        AgentKind::Amp,
        AgentKind::Codex,
        AgentKind::Hermes,
    ] {
        let (dir, worker) = setup(kind);
        for input in ["policy error", "missing end", "bad exit", "tool violation"] {
            let id =
                question::launch_with_agent(dir.path(), &worker, None, input, &selection(kind))
                    .unwrap();
            let q = wait(dir.path(), &id);
            assert_eq!(
                q.turns[0].state,
                State::Stopped,
                "{kind:?} {input}: {:?}",
                q.turns[0]
            );
            assert!(q.turns[0].error.is_some());
        }
    }
}
#[test]
fn cli_adapters_resume_bound_clarification_and_reject_duplicate_replies() {
    for kind in [
        AgentKind::Copilot,
        AgentKind::Amp,
        AgentKind::Codex,
        AgentKind::Hermes,
    ] {
        let (dir, worker) = setup(kind);
        let id =
            question::launch_with_agent(dir.path(), &worker, None, "clarify", &selection(kind))
                .unwrap();
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
        assert_eq!(next.agent, selection(kind));
        assert!(question::respond(dir.path(), &worker, &reply).is_err());
    }
}
#[test]
fn cli_adapters_can_be_cancelled() {
    for kind in [
        AgentKind::Copilot,
        AgentKind::Amp,
        AgentKind::Codex,
        AgentKind::Hermes,
    ] {
        let (dir, worker) = setup(kind);
        let id = question::launch_with_agent(dir.path(), &worker, None, "slow", &selection(kind))
            .unwrap();
        question::cancel(dir.path(), &id).unwrap();
        assert_eq!(wait(dir.path(), &id).turns[0].state, State::Cancelled);
    }
}
