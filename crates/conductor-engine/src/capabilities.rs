//! Native discovery, dispatch and retained output. No agent is called by this module.
use crate::store;
use conductor_checks::runner;
use conductor_model::{
    capability::{
        Available, Binding, CAPTURE_LIMIT, Capture, Descriptor, Event, EventKind, NativeView,
        Record, VERSION, valid_id,
    },
    task::{Activity, State, TaskDetail, TaskSummary},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub trait Adapter: Send + Sync {
    fn name(&self) -> &str;
    fn describe(&self, repo: &Path, input: &Value) -> Result<String, String>;
    fn execute(&self, repo: &Path, input: &Value) -> Result<Capture, String>;
}

#[derive(Default)]
pub struct Registry {
    adapters: BTreeMap<String, Arc<dyn Adapter>>,
    descriptors: BTreeMap<String, Descriptor>,
}
impl Registry {
    pub fn register_adapter(&mut self, adapter: Arc<dyn Adapter>) -> Result<(), String> {
        if self.adapters.contains_key(adapter.name()) {
            return Err("Adapter already registered.".into());
        }
        self.adapters.insert(adapter.name().into(), adapter);
        Ok(())
    }
    pub fn admit(&mut self, descriptor: Descriptor) -> Result<(), String> {
        descriptor.validate()?;
        if self.descriptors.contains_key(&descriptor.id) {
            return Err(format!("Duplicate capability: {}", descriptor.id));
        }
        self.descriptors.insert(descriptor.id.clone(), descriptor);
        Ok(())
    }
    pub fn load(repo: &Path) -> Result<Self, String> {
        let mut r = Self::default();
        r.register_adapter(Arc::new(FileAdapter))?;
        r.register_adapter(Arc::new(GitAdapter))?;
        r.admit(Descriptor {
            version: VERSION,
            id: "repo.status".into(),
            label: "Repository changes".into(),
            adapter: "git.status.v1".into(),
            input: json!({}),
        })?;
        let dir = repo.join(".conductor/capabilities");
        if !dir.exists() {
            return Ok(r);
        }
        let mut paths = fs::read_dir(dir)
            .map_err(err)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        paths.sort();
        for path in paths
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
        {
            let data = read_bounded(&path, CAPTURE_LIMIT)?;
            let d: Descriptor =
                serde_json::from_slice(&data).map_err(|e| format!("{}: {e}", path.display()))?;
            r.admit(d)?;
        }
        Ok(r)
    }
    pub fn available(&self, repo: &Path) -> Vec<Available> {
        self.descriptors
            .values()
            .map(|d| {
                let detail = self
                    .adapters
                    .get(&d.adapter)
                    .ok_or_else(|| format!("Adapter {} is not installed.", d.adapter))
                    .and_then(|a| a.describe(repo, &d.input));
                Available {
                    binding: d.binding(),
                    label: d.label.clone(),
                    detail: detail.clone().unwrap_or_else(|_| d.adapter.clone()),
                    unavailable: detail.err(),
                }
            })
            .collect()
    }
    pub fn descriptor(&self, binding: &Binding) -> Result<&Descriptor, String> {
        let d = self
            .descriptors
            .get(&binding.id)
            .ok_or("Capability removed. Saved results remain available.")?;
        if d.binding() != *binding {
            return Err("Capability changed. Reopen Actions and start with its new definition; the saved task keeps its original binding.".into());
        }
        Ok(d)
    }
    pub fn execute(&self, repo: &Path, binding: &Binding) -> Result<Capture, String> {
        let d = self.descriptor(binding)?;
        let a = self
            .adapters
            .get(&d.adapter)
            .ok_or("The capability's adapter is unavailable.")?;
        a.describe(repo, &d.input)?;
        a.execute(repo, &d.input)
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    if !fs::metadata(path).map_err(err)?.is_file() {
        return Err("Select a regular file.".into());
    }
    let file = File::open(path).map_err(err)?;
    if !file.metadata().map_err(err)?.is_file() {
        return Err("Select a regular file.".into());
    }
    let mut data = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(err)?;
    if data.len() > limit {
        return Err(format!(
            "{} exceeds the {} KiB limit; no partial result was accepted.",
            path.display(),
            limit / 1024
        ));
    }
    Ok(data)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileInput {
    path: String,
}
struct FileAdapter;
fn selected_file(repo: &Path, input: &Value) -> Result<PathBuf, String> {
    let input: FileInput = serde_json::from_value(input.clone()).map_err(err)?;
    let relative = Path::new(&input.path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Report paths must name a file within this repository.".into());
    }
    let root = fs::canonicalize(repo).map_err(err)?;
    let path = fs::canonicalize(root.join(relative)).map_err(err)?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("Report is unavailable or leaves this repository.".into());
    }
    Ok(path)
}
impl Adapter for FileAdapter {
    fn name(&self) -> &str {
        "file.read.v1"
    }
    fn describe(&self, repo: &Path, input: &Value) -> Result<String, String> {
        selected_file(repo, input)?;
        Ok(format!(
            "Read {} · JSON or text · no agent",
            input["path"].as_str().unwrap_or_default()
        ))
    }
    fn execute(&self, repo: &Path, input: &Value) -> Result<Capture, String> {
        let path = selected_file(repo, input)?;
        let data = read_bounded(&path, CAPTURE_LIMIT)?;
        // Recheck a changed symlink before accepting its bytes. This adapter is a local
        // selected-file reader, not a sandbox for hostile concurrent filesystem writers.
        if selected_file(repo, input)? != path {
            return Err(
                "The selected path changed while reading it. Retry with a stable file.".into(),
            );
        }
        let body = String::from_utf8(data)
            .map_err(|_| "The selected file is not UTF-8 text.".to_owned())?;
        let media = if serde_json::from_str::<Value>(&body).is_ok() {
            "application/json"
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
        {
            "text/markdown"
        } else {
            "text/plain"
        };
        Capture::new(path.display().to_string(), media.into(), body)
    }
}
struct GitAdapter;
impl Adapter for GitAdapter {
    fn name(&self) -> &str {
        "git.status.v1"
    }
    fn describe(&self, _repo: &Path, input: &Value) -> Result<String, String> {
        if input != &json!({}) {
            return Err("Repository changes takes no inputs.".into());
        }
        Ok("Read working-tree status · no file edits or agent".into())
    }
    fn execute(&self, repo: &Path, _input: &Value) -> Result<Capture, String> {
        let argv: Vec<String> = [
            "git",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=all",
            "--",
            ".",
            ":(exclude).conductor/native-tasks",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let run = runner::run(&runner::Spec {
            argv: &argv,
            cwd: repo,
            timeout: Duration::from_secs(10),
            pass_env: &[],
            run_id: "native-status",
            inherit_env: false,
            set_env: &[],
        });
        if !run.complete() || run.exit_code != Some(0) {
            return Err(format!(
                "Git status failed: {} {}",
                run.reason.unwrap_or_default(),
                run.stderr_tail
            ));
        }
        if run.stdout.len() > CAPTURE_LIMIT {
            return Err("Git status exceeds the 64 KiB capture limit.".into());
        }
        let text = String::from_utf8(run.stdout)
            .map_err(|_| "Git returned paths that are not UTF-8.".to_owned())?;
        let mut records = vec![];
        let mut fields = text.split('\0').filter(|s| !s.is_empty());
        while let Some(field) = fields.next() {
            let status = field.get(..2).ok_or("Malformed Git status record.")?;
            let path = field.get(3..).ok_or("Malformed Git status path.")?;
            let mut row = json!({"status":status,"path":path});
            if status.contains(['R', 'C']) {
                row["previous_path"] = json!(fields.next().ok_or("Missing rename source.")?);
            }
            records.push(row);
        }
        let body = if records.is_empty() {
            json!({"Working tree":"Clean"}).to_string()
        } else {
            serde_json::to_string_pretty(&records).map_err(err)?
        };
        Capture::new(
            "git status --porcelain=v1 (native adapter)".into(),
            "application/json".into(),
            body,
        )
    }
}

/// Explicit registration of a selected report; never discovers executable commands from repo text.
pub fn add_report(repo: &Path, id: &str, label: &str, file: &Path) -> Result<Descriptor, String> {
    let root = fs::canonicalize(repo).map_err(err)?;
    let path = fs::canonicalize(file).map_err(err)?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "Select a report inside this repository.".to_owned())?;
    let d = Descriptor {
        version: VERSION,
        id: id.into(),
        label: label.into(),
        adapter: "file.read.v1".into(),
        input: json!({"path":relative.to_str().ok_or("Report path is not UTF-8.")?}),
    };
    d.validate()?;
    if id == "repo.status" {
        return Err("repo.status is provided by the native Git adapter.".into());
    }
    FileAdapter.describe(repo, &d.input)?;
    let dir = repo.join(".conductor/capabilities");
    fs::create_dir_all(&dir).map_err(err)?;
    // create_new prevents accidentally replacing an existing descriptor/binding.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("{id}.json")))
        .map_err(err)?;
    output
        .write_all(&serde_json::to_vec_pretty(&d).map_err(err)?)
        .map_err(err)?;
    output.sync_all().map_err(err)?;
    Ok(d)
}
pub fn remove(repo: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) || id == "repo.status" {
        return Err("Select a registered report operation.".into());
    }
    fs::remove_file(
        repo.join(".conductor/capabilities")
            .join(format!("{id}.json")),
    )
    .map_err(err)
}

fn directory(repo: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) || !id.starts_with("n-") {
        return Err("Invalid native task identifier.".into());
    }
    Ok(repo.join(".conductor/native-tasks").join(id))
}
fn save(dir: &Path, record: &Record) -> Result<(), String> {
    record.project()?;
    let mut file = File::create(dir.join("record.tmp")).map_err(err)?;
    serde_json::to_writer(&mut file, record).map_err(err)?;
    file.sync_all().map_err(err)?;
    fs::rename(dir.join("record.tmp"), dir.join("record.json")).map_err(err)
}
pub fn read(repo: &Path, id: &str) -> Result<Record, String> {
    let data = read_bounded(&directory(repo, id)?.join("record.json"), 16 * 1024 * 1024)?;
    let record: Record = serde_json::from_slice(&data).map_err(err)?;
    if record.id != id {
        return Err("Task identifier does not match its record.".into());
    }
    record.project()?;
    Ok(record)
}
pub fn list(repo: &Path) -> Vec<Record> {
    let mut records: Vec<_> = fs::read_dir(repo.join(".conductor/native-tasks"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| read(repo, &e.file_name().to_string_lossy()).ok())
        .collect();
    records.sort_by(|a, b| b.id.cmp(&a.id));
    records
}

fn reserve(dir: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("busy"))
        .map_err(err)?;
    file.try_lock().map_err(|_| {
        "This native task is already working. Wait for its current read to finish.".to_owned()
    })?;
    Ok(file) // OS lock is released on drop, including process exit.
}
fn active(repo: &Path, id: &str) -> bool {
    directory(repo, id)
        .ok()
        .and_then(|dir| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.join("busy"))
                .ok()
        })
        .is_some_and(|file| file.try_lock().is_err())
}
fn event(record: &mut Record, kind: EventKind) {
    record.events.push(Event {
        seq: record.events.len(),
        at: store::now(),
        kind,
    });
}

pub fn invoke(repo: &Path, binding: &Binding, parent: Option<&str>) -> Result<String, String> {
    let registry = Registry::load(repo)?;
    let descriptor = registry.descriptor(binding)?.clone();
    let id = parent
        .map(str::to_owned)
        .unwrap_or_else(|| format!("n-{}", store::new_run_id()));
    let dir = directory(repo, &id)?;
    if parent.is_none() {
        fs::create_dir_all(dir.parent().unwrap()).map_err(err)?;
        fs::create_dir(&dir).map_err(err)?;
    }
    let _reservation = reserve(&dir)?;
    let mut record = if parent.is_some() {
        read(repo, &id)?
    } else {
        Record {
            version: VERSION,
            id: id.clone(),
            binding: binding.clone(),
            descriptor,
            events: vec![],
        }
    };
    if record.binding != *binding {
        return Err("A saved task cannot switch its capability binding.".into());
    }
    let p = record.project()?;
    if p.attempts >= 32 {
        return Err("This task has 32 attempts. Start a new task to retain more results.".into());
    }
    if p.pending {
        event(
            &mut record,
            EventKind::Failed {
                attempt: p.attempts,
                error: "The previous read was interrupted; no completion was recorded.".into(),
            },
        );
    }
    let attempt = p.attempts + 1;
    event(&mut record, EventKind::Started { attempt });
    save(&dir, &record)?;
    // Revalidate the descriptor immediately before dispatch; stale UI bindings cannot
    // silently select a different input, implementation or connection.
    let result = Registry::load(repo).and_then(|r| r.execute(repo, binding));
    event(
        &mut record,
        match result {
            Ok(output) => EventKind::Captured { attempt, output },
            Err(error) => EventKind::Failed { attempt, error },
        },
    );
    save(&dir, &record)?;
    Ok(id)
}

pub fn detail(repo: &Path, record: &Record) -> Result<TaskDetail, String> {
    let p = record.project()?;
    let refresh = Registry::load(repo).and_then(|r| {
        r.descriptor(&record.binding)?;
        let item = r
            .available(repo)
            .into_iter()
            .find(|a| a.binding == record.binding)
            .ok_or("Capability unavailable.")?;
        if let Some(why) = item.unavailable {
            return Err(why);
        }
        Ok(record.binding.clone())
    });
    let running = p.pending && active(repo, &record.id);
    let state = if running {
        State::Working
    } else if p.pending || p.error.is_some() {
        State::Stopped
    } else {
        State::Finished
    };
    let mut evidence = vec![];
    for e in record.events.iter().rev() {
        if let EventKind::Captured { attempt, output } = &e.kind {
            evidence.push((
                format!("Captured result · attempt {attempt}"),
                format!(
                    "Source: {}\nCaptured: {}\nHash: {}\n\n{}",
                    output.source, e.at, output.hash, output.body
                ),
            ));
        }
    }
    evidence.push(("Capability binding".into(),format!("{}\n{}\nAdapter: {}\nInputs: {}\n\nNative capture. External report claims are data, not independent test verification. These task records are not signed or Git-anchored receipts.",record.descriptor.id,record.binding.fingerprint,record.descriptor.adapter,record.descriptor.input)));
    let notice = if running {
        Some("Reading the selected source…".into())
    } else if p.pending {
        Some("The previous read was interrupted. r retries explicitly; reopening this record never runs work.".into())
    } else if let Some(error) = &p.error {
        Some(format!(
            "Attempt {} failed: {error}{}",
            p.attempts,
            if p.output.is_some() {
                format!(" · Showing retained attempt {}.", p.output_attempt)
            } else {
                String::new()
            }
        ))
    } else {
        refresh
            .as_ref()
            .err()
            .map(|e| format!("Refresh unavailable: {e}"))
    };
    let activity = record
        .events
        .iter()
        .map(|e| {
            let (title, detail, failed) = match &e.kind {
                EventKind::Started { attempt } => (
                    format!("Native read started · attempt {attempt}"),
                    format!(
                        "{} · {}",
                        record.descriptor.adapter, record.binding.fingerprint
                    ),
                    false,
                ),
                EventKind::Captured { attempt, output } => (
                    format!("Result captured · attempt {attempt}"),
                    format!(
                        "{}\n{} bytes · {}\n0 agent calls",
                        output.source,
                        output.body.len(),
                        output.hash
                    ),
                    false,
                ),
                EventKind::Failed { attempt, error } => (
                    format!("Read failed · attempt {attempt}"),
                    error.clone(),
                    true,
                ),
            };
            Activity {
                at: e.at.clone(),
                actor: "Conductor".into(),
                title,
                detail,
                failed,
            }
        })
        .collect();
    Ok(TaskDetail {
        receipt: None,
        summary: TaskSummary {
            id: record.id.clone(),
            title: record.descriptor.label.clone(),
            kind: "Native operation".into(),
            state,
        },
        context: format!(
            "{} · {} attempt(s) · 0 agent calls",
            record.descriptor.id, p.attempts
        ),
        body: p
            .output
            .as_ref()
            .map(|o| o.body.clone())
            .unwrap_or_else(|| "No result captured yet.".into()),
        evidence,
        activity,
        waiting: None,
        question: None,
        tests: None,
        changes: vec![],
        native: Some(NativeView {
            media_type: p
                .output
                .as_ref()
                .map(|o| {
                    if o.media_type == "text/plain"
                        && Path::new(&o.source).extension().is_some_and(|e| {
                            e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")
                        })
                    {
                        "text/markdown".into()
                    } else {
                        o.media_type.clone()
                    }
                })
                .unwrap_or_else(|| "text/plain".into()),
            answer: p
                .output
                .as_ref()
                .map(|o| o.answer(&record.descriptor.label)),
            refresh: refresh.ok(),
            notice,
        }),
    })
}
