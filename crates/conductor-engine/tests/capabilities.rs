use conductor_engine::capabilities::{self, Adapter, Registry};
use conductor_model::capability::{Capture, Descriptor, EventKind};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct CounterAdapter {
    name: &'static str,
    calls: Arc<AtomicUsize>,
}
impl Adapter for CounterAdapter {
    fn name(&self) -> &str {
        self.name
    }
    fn describe(&self, _: &Path, _: &serde_json::Value) -> Result<String, String> {
        Ok("Fixture adapter".into())
    }
    fn execute(&self, _: &Path, input: &serde_json::Value) -> Result<Capture, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Capture::new(
            self.name.into(),
            "application/json".into(),
            input.to_string(),
        )
    }
}

#[test]
fn runtime_registration_uses_the_existing_renderer_and_rejects_stale_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut r = Registry::default();
    for name in ["first.v1", "second.v1"] {
        r.register_adapter(Arc::new(CounterAdapter {
            name,
            calls: calls.clone(),
        }))
        .unwrap();
        r.admit(Descriptor {
            version: 1,
            id: name.into(),
            label: name.into(),
            adapter: name.into(),
            input: json!([{"test":"account", "status":"failed"}]),
        })
        .unwrap();
    }
    assert_eq!(r.available(dir.path()).len(), 2);
    for item in r.available(dir.path()) {
        assert!(
            r.execute(dir.path(), &item.binding)
                .unwrap()
                .answer(&item.label)
                .presentation()
                .unwrap()
                .is_some()
        );
        let mut stale = item.binding;
        stale.fingerprint = "changed".into();
        assert!(r.execute(dir.path(), &stale).is_err());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn refresh_retains_outputs_and_reopening_after_removal_never_executes() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.json");
    fs::write(&report, r#"[{"name":"lowercase","status":"failed"}]"#).unwrap();
    let d = capabilities::add_report(dir.path(), "qa.failures", "QA report", &report).unwrap();
    let id = capabilities::invoke(dir.path(), &d.binding(), None).unwrap();
    let first = capabilities::read(dir.path(), &id).unwrap();
    assert!(
        first
            .project()
            .unwrap()
            .output
            .unwrap()
            .body
            .contains("failed")
    );
    fs::write(&report, r#"[{"name":"lowercase","status":"passed"}]"#).unwrap();
    capabilities::invoke(dir.path(), &d.binding(), Some(&id)).unwrap();
    fs::remove_file(&report).unwrap();
    capabilities::invoke(dir.path(), &d.binding(), Some(&id)).unwrap();
    let record = capabilities::read(dir.path(), &id).unwrap();
    let p = record.project().unwrap();
    assert_eq!(p.attempts, 3);
    assert_eq!(p.output_attempt, 2);
    assert!(p.output.unwrap().body.contains("passed"));
    assert!(p.error.is_some());
    assert_eq!(&record.events[..2], first.events);
    assert!(
        record
            .events
            .iter()
            .any(|e| matches!(&e.kind, EventKind::Failed { attempt: 3, .. }))
    );
    capabilities::remove(dir.path(), "qa.failures").unwrap();
    for _ in 0..3 {
        let view = capabilities::detail(dir.path(), &capabilities::read(dir.path(), &id).unwrap())
            .unwrap();
        assert!(view.native.unwrap().refresh.is_none());
        assert!(view.tests.is_none()); // imported "passed" is never a witnessed test verdict
        assert!(view.question.is_none());
        assert!(view.evidence[0].1.contains("passed"));
    }
    assert_eq!(record, capabilities::read(dir.path(), &id).unwrap());
    assert!(capabilities::invoke(dir.path(), &d.binding(), Some(&id)).is_err());
}

#[test]
fn rebinding_a_name_never_retargets_a_saved_task() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "old input").unwrap();
    fs::write(&b, "different input").unwrap();
    let old = capabilities::add_report(dir.path(), "custom", "Custom", &a).unwrap();
    let id = capabilities::invoke(dir.path(), &old.binding(), None).unwrap();
    capabilities::remove(dir.path(), "custom").unwrap();
    let new = capabilities::add_report(dir.path(), "custom", "Custom", &b).unwrap();
    assert!(capabilities::invoke(dir.path(), &old.binding(), Some(&id)).is_err());
    assert!(capabilities::invoke(dir.path(), &new.binding(), Some(&id)).is_err());
    assert_eq!(
        capabilities::read(dir.path(), &id)
            .unwrap()
            .project()
            .unwrap()
            .attempts,
        1
    );
}

#[test]
fn paths_large_reports_and_unknown_schemas_fail_without_hiding_old_results() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("secret.txt");
    fs::write(&path, "private").unwrap();
    assert!(capabilities::add_report(dir.path(), "external", "External", &path).is_err());
    let path = dir.path().join("report.txt");
    fs::write(&path, "original").unwrap();
    let d = capabilities::add_report(dir.path(), "report", "Report", &path).unwrap();
    let id = capabilities::invoke(dir.path(), &d.binding(), None).unwrap();
    fs::write(&path, vec![b'x'; 65537]).unwrap();
    capabilities::invoke(dir.path(), &d.binding(), Some(&id)).unwrap();
    let mut record = capabilities::read(dir.path(), &id).unwrap();
    assert_eq!(record.project().unwrap().output.unwrap().body, "original");
    record.version = 99;
    assert!(record.project().is_err());
    assert!(capabilities::read(dir.path(), "../other").is_err());
    #[cfg(unix)]
    {
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), &path).unwrap();
        capabilities::invoke(dir.path(), &d.binding(), Some(&id)).unwrap();
        let p = capabilities::read(dir.path(), &id)
            .unwrap()
            .project()
            .unwrap();
        assert_eq!(p.output.unwrap().body, "original");
        assert!(p.error.is_some());
    }
}

#[test]
fn git_status_runs_without_an_agent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(dir.path().join("new file.txt"), "hello").unwrap();
    let binding = Registry::load(dir.path())
        .unwrap()
        .available(dir.path())
        .remove(0)
        .binding;
    let id = capabilities::invoke(dir.path(), &binding, None).unwrap();
    let record = capabilities::read(dir.path(), &id).unwrap();
    assert!(
        record
            .project()
            .unwrap()
            .output
            .unwrap()
            .body
            .contains("new file.txt")
    );
    assert!(
        capabilities::detail(dir.path(), &record)
            .unwrap()
            .context
            .contains("0 agent calls")
    );
}

#[test]
fn writer_lock_prevents_duplicate_work_and_interrupted_reads_require_an_explicit_retry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.txt");
    fs::write(&file, "retained").unwrap();
    let d = capabilities::add_report(dir.path(), "report", "Report", &file).unwrap();
    let id = capabilities::invoke(dir.path(), &d.binding(), None).unwrap();
    let task_dir = dir.path().join(".conductor/native-tasks").join(&id);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(task_dir.join("busy"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(capabilities::invoke(dir.path(), &d.binding(), Some(&id)).is_err());
    assert_eq!(capabilities::read(dir.path(), &id).unwrap().events.len(), 2);
    drop(lock);
    let mut record = capabilities::read(dir.path(), &id).unwrap();
    record.events.push(conductor_model::capability::Event {
        seq: 2,
        at: "interrupted".into(),
        kind: EventKind::Started { attempt: 2 },
    });
    fs::write(
        task_dir.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let view = capabilities::detail(dir.path(), &record).unwrap();
    assert_eq!(view.summary.state, conductor_model::task::State::Stopped);
    assert!(view.native.unwrap().notice.unwrap().contains("interrupted"));
    assert_eq!(capabilities::read(dir.path(), &id).unwrap(), record);
    capabilities::invoke(dir.path(), &d.binding(), Some(&id)).unwrap();
    let after = capabilities::read(dir.path(), &id).unwrap();
    assert!(matches!(
        &after.events[3].kind,
        EventKind::Failed { attempt: 2, .. }
    ));
    assert_eq!(after.project().unwrap().attempts, 3);
    assert_eq!(after.project().unwrap().output_attempt, 3);
}

#[test]
fn markdown_reports_keep_their_format_and_old_captures_still_render_as_documents() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.md");
    fs::write(&file, "# Notes\n\n**A saved finding**").unwrap();
    let d = capabilities::add_report(dir.path(), "notes", "Notes", &file).unwrap();
    let id = capabilities::invoke(dir.path(), &d.binding(), None).unwrap();
    let mut record = capabilities::read(dir.path(), &id).unwrap();
    assert_eq!(
        record.project().unwrap().output.unwrap().media_type,
        "text/markdown"
    );
    assert_eq!(
        capabilities::detail(dir.path(), &record)
            .unwrap()
            .native
            .unwrap()
            .media_type,
        "text/markdown"
    );
    if let EventKind::Captured { output, .. } = &mut record.events[1].kind {
        output.media_type = "text/plain".into();
    }
    assert_eq!(
        capabilities::detail(dir.path(), &record)
            .unwrap()
            .native
            .unwrap()
            .media_type,
        "text/markdown"
    );
}
