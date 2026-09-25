//! Runs a command a workflow declared, in conductor's own process, and records what happened.
//!
//! Ported from dock's check runner, which earned each of these properties the hard way:
//!
//! * **Both pipes are drained while the child runs.** A command that fills a pipe buffer
//!   while conductor waits on it would deadlock both sides.
//! * **A timeout kills the process group**, not just the process conductor started, so a
//!   test harness's own children don't outlive it.
//! * **Nothing waits without a deadline.** A command that leaves a descendant holding its
//!   output open can't park conductor.
//! * **The environment starts empty.** A command sees an allowlist of ambient variables plus
//!   the ones the workflow names, so a credential in conductor's environment doesn't reach
//!   code an agent just wrote.
//!
//! Unlike dock, stdout is kept whole (up to a cap), because gates parse it; both streams are
//! hashed in full, so the receipt can prove which output a verdict was read from.

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Stdout beyond this is hashed but not kept. A gate reading a truncated stream is told so.
pub const STDOUT_CAP: usize = 32 * 1024 * 1024;
/// How many trailing lines of stderr the receipt keeps for a person to read.
pub const TAIL_LINES: usize = 60;
/// How long a timed-out group gets between SIGTERM and SIGKILL, and how long conductor waits
/// for the pipes to close once the command has exited.
const GRACE: Duration = Duration::from_secs(5);

/// Ambient variables a command may inherit. Everything else is dropped. Beyond the basics,
/// these say where a language's toolchain and caches live, so a check finds the same
/// compiler, virtualenv or module cache the developer uses; none of them carries a secret.
pub fn inherits(key: &str) -> bool {
    matches!(
        key,
        "HOME"
            | "LANG"
            | "LOGNAME"
            | "PATH"
            | "SHELL"
            | "TMPDIR"
            | "USER"
            | "XDG_CACHE_HOME"
            // Rust
            | "CARGO_HOME"
            | "RUSTUP_HOME"
            | "RUSTUP_TOOLCHAIN"
            // Go
            | "GOROOT"
            | "GOPATH"
            | "GOCACHE"
            | "GOMODCACHE"
            | "GOTOOLCHAIN"
            // Python
            | "VIRTUAL_ENV"
            | "CONDA_PREFIX"
            | "PYENV_ROOT"
            | "PYENV_VERSION"
            // JavaScript
            | "NVM_DIR"
            | "PNPM_HOME"
            | "VOLTA_HOME"
            // JVM
            | "JAVA_HOME"
            | "GRADLE_USER_HOME"
            | "MAVEN_HOME"
            | "M2_HOME"
            // .NET, Ruby
            | "DOTNET_ROOT"
            | "GEM_HOME"
            | "GEM_PATH"
            | "RBENV_ROOT"
    ) || key.starts_with("LC_")
}

#[derive(Debug, Clone)]
pub struct Spec<'a> {
    pub argv: &'a [String],
    pub cwd: &'a Path,
    pub timeout: Duration,
    /// Extra variable names the workflow asked to pass through.
    pub pass_env: &'a [String],
    pub run_id: &'a str,
    /// Keep the whole ambient environment. Only for agents, which need their own
    /// credentials; checks never get this.
    pub inherit_env: bool,
    /// Extra variables to set, such as the prompt for a scripted agent.
    pub set_env: &'a [(String, String)],
}

/// Variables that tie a Claude Code process to the session that launched it. An agent
/// conductor starts must be its own session, never a child that resumes or reports into
/// whoever ran conductor, so these are removed even when the rest is inherited.
pub fn couples_to_parent_session(key: &str) -> bool {
    matches!(key, "CLAUDECODE" | "CLAUDE_PID" | "AI_AGENT")
        || [
            "CLAUDE_CODE_SESSION",
            "CLAUDE_CODE_CHILD",
            "CLAUDE_CODE_REMOTE_SESSION",
            "CLAUDE_CODE_MESSAGING",
            "CLAUDE_CODE_ARTIFACT",
            "CLAUDE_CODE_SYNC",
            "CLAUDE_CODE_TEE",
            "CLAUDE_CODE_BG_",
            "CLAUDE_CODE_DIAGNOSTICS",
            "CLAUDE_AUTO_BACKGROUND",
            "CLAUDE_AFTER_LAST_COMPACT",
            "CLAUDE_CODE_POST_FOR",
            "CLAUDE_CODE_HOLD_",
            "CLAUDE_CODE_WORKER",
            "CLAUDE_CODE_BASE_REF",
        ]
        .iter()
        .any(|p| key.starts_with(p))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    Exited,
    TimedOut,
    /// The command never started, or conductor lost track of it. Nothing was witnessed.
    NotRun,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Execution {
    pub argv: Vec<String>,
    pub ended: Ended,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(skip)]
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stderr_tail: String,
    /// Why nothing was witnessed, when that is the case, or why the output is incomplete.
    pub reason: Option<String>,
}

impl Execution {
    fn not_run(argv: &[String], reason: String) -> Self {
        let empty = format!("sha256:{}", hex::encode(Sha256::digest([])));
        Execution {
            argv: argv.to_vec(),
            ended: Ended::NotRun,
            exit_code: None,
            duration_ms: 0,
            stdout: Vec::new(),
            stdout_truncated: false,
            stdout_sha256: empty.clone(),
            stderr_sha256: empty,
            stderr_tail: String::new(),
            reason: Some(reason),
        }
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Whether the command ran to an exit and conductor saw all of its output.
    pub fn complete(&self) -> bool {
        self.ended == Ended::Exited && !self.stdout_truncated && self.reason.is_none()
    }
}

struct Captured {
    kept: Vec<u8>,
    truncated: bool,
    sha256: String,
    tail: Vec<String>,
}

/// Reads a stream to its end: hashes all of it, keeps what fits, and, when `lines` is
/// given, hands over each complete line as it arrives.
fn capture(mut stream: impl Read, keep_all: bool, lines: Option<mpsc::Sender<String>>) -> Captured {
    let mut hasher = Sha256::new();
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buf = [0_u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        hasher.update(&buf[..n]);
        if let Some(tx) = &lines {
            pending.extend_from_slice(&buf[..n]);
            while let Some(i) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=i).collect();
                let _ = tx.send(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
            }
        }
        let room = if keep_all {
            STDOUT_CAP.saturating_sub(kept.len())
        } else {
            usize::MAX
        };
        if n > room {
            kept.extend_from_slice(&buf[..room]);
            truncated = true;
        } else {
            kept.extend_from_slice(&buf[..n]);
        }
        if !keep_all && kept.len() > 1024 * 1024 {
            // Only the tail matters for stderr; keep memory bounded.
            let cut = kept.len() - 256 * 1024;
            kept.drain(..cut);
        }
    }
    if let Some(tx) = &lines
        && !pending.is_empty()
    {
        let _ = tx.send(String::from_utf8_lossy(&pending).into_owned());
    }
    let tail = if keep_all {
        Vec::new()
    } else {
        let text = String::from_utf8_lossy(&kept);
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(TAIL_LINES)..]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    };
    Captured {
        kept,
        truncated,
        sha256: format!("sha256:{}", hex::encode(hasher.finalize())),
        tail,
    }
}

pub fn run(spec: &Spec) -> Execution {
    run_impl(spec, None)
}

/// Like [`run`], and `on_line` sees each line of stdout as the command writes it, so a
/// long-running agent can be watched. Everything else, the hashes, the timeout, the
/// group kill, is the same.
pub fn run_streaming(spec: &Spec, on_line: &mut dyn FnMut(&str)) -> Execution {
    run_impl(spec, Some(on_line))
}

/// How often the wait loop looks for new lines.
const TICK: Duration = Duration::from_millis(50);

fn run_impl(spec: &Spec, mut on_line: Option<&mut dyn FnMut(&str)>) -> Execution {
    let Some(program) = spec.argv.first().filter(|p| !p.trim().is_empty()) else {
        return Execution::not_run(spec.argv, "the command is empty".into());
    };
    let mut command = Command::new(program);
    command
        .args(&spec.argv[1..])
        .current_dir(spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if spec.inherit_env {
        for (k, _) in std::env::vars().filter(|(k, _)| couples_to_parent_session(k)) {
            command.env_remove(k);
        }
    } else {
        command
            .env_clear()
            .envs(std::env::vars().filter(|(k, _)| inherits(k)));
    }
    for (k, v) in spec.set_env {
        command.env(k, v);
    }
    for name in spec.pass_env {
        if let Ok(v) = std::env::var(name) {
            command.env(name, v);
        }
    }
    command.env("CONDUCTOR_RUN_ID", spec.run_id);

    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Execution::not_run(spec.argv, format!("could not start `{program}`: {e}"));
        }
    };
    let group = Pid::from_raw(child.id().cast_signed());

    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    let (line_tx, line_rx) = mpsc::channel::<String>();
    let lines = on_line.is_some().then_some(line_tx);
    if let Some(s) = child.stdout.take() {
        std::thread::spawn(move || {
            let _ = out_tx.send(capture(s, true, lines));
        });
    }
    if let Some(s) = child.stderr.take() {
        std::thread::spawn(move || {
            let _ = err_tx.send(capture(s, false, None));
        });
    }

    let (reaped, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = reaped.send(child.wait());
    });

    // Wait for the exit, with a deadline, handing over stdout lines meanwhile. Nothing here
    // blocks without one.
    let deadline = started + spec.timeout;
    let (ended, exit_code, mut reason) = loop {
        if let Some(f) = &mut on_line {
            while let Ok(l) = line_rx.try_recv() {
                f(&l);
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match exited.recv_timeout(left.min(TICK)) {
            Ok(Ok(status)) => break (Ended::Exited, status.code(), None),
            Ok(Err(e)) => {
                break (
                    Ended::NotRun,
                    None,
                    Some(format!("could not wait for `{program}`: {e}")),
                );
            }
            Err(RecvTimeoutError::Disconnected) => {
                break (
                    Ended::NotRun,
                    None,
                    Some(format!("lost track of `{program}` while it ran")),
                );
            }
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                for signal in [Signal::SIGTERM, Signal::SIGKILL] {
                    let _ = killpg(group, signal);
                    if exited.recv_timeout(GRACE).is_ok() {
                        break;
                    }
                }
                break (
                    Ended::TimedOut,
                    None,
                    Some(format!("timed out after {}s", spec.timeout.as_secs())),
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let out = out_rx.recv_timeout(GRACE).ok();
    if let Some(f) = &mut on_line {
        // Every line the reader sent is queued before it hands over the capture.
        while let Ok(l) = line_rx.try_recv() {
            f(&l);
        }
    }
    let err = err_rx.recv_timeout(GRACE).ok();
    if (out.is_none() || err.is_none()) && reason.is_none() {
        reason = Some("the command exited but something it started kept its output open".into());
    }
    let out = out.unwrap_or(Captured {
        kept: Vec::new(),
        truncated: true,
        sha256: String::new(),
        tail: Vec::new(),
    });
    let err = err.unwrap_or(Captured {
        kept: Vec::new(),
        truncated: true,
        sha256: String::new(),
        tail: Vec::new(),
    });

    Execution {
        argv: spec.argv.to_vec(),
        ended,
        exit_code,
        duration_ms,
        stdout: out.kept,
        stdout_truncated: out.truncated,
        stdout_sha256: out.sha256,
        stderr_sha256: err.sha256,
        stderr_tail: err.tail.join("\n"),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Duration) -> Execution {
        let argv = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
        let dir = std::env::temp_dir();
        run(&Spec {
            argv: &argv,
            cwd: &dir,
            timeout,
            pass_env: &[],
            run_id: "r1",
            inherit_env: false,
            set_env: &[],
        })
    }

    #[test]
    fn exit_code_and_output_are_recorded() {
        let e = sh("echo out; echo err >&2; exit 3", Duration::from_secs(10));
        assert_eq!(e.ended, Ended::Exited);
        assert_eq!(e.exit_code, Some(3));
        assert_eq!(e.stdout_text(), "out\n");
        assert_eq!(e.stderr_tail, "err");
        assert!(e.complete());
        let expected = format!("sha256:{}", hex::encode(Sha256::digest(b"out\n")));
        assert_eq!(e.stdout_sha256, expected);
    }

    #[test]
    fn a_loud_command_does_not_deadlock() {
        let e = sh("seq 1 200000; seq 1 200000 >&2", Duration::from_secs(30));
        assert_eq!(e.exit_code, Some(0));
        assert!(e.stdout.len() > 1_000_000);
        assert!(e.stderr_tail.ends_with("200000"));
        assert!(e.stderr_tail.lines().count() <= TAIL_LINES);
    }

    #[test]
    fn lines_arrive_while_the_command_runs() {
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo first; sleep 1; echo second".to_string(),
        ];
        let dir = std::env::temp_dir();
        let started = Instant::now();
        let mut seen: Vec<(String, Duration)> = Vec::new();
        let e = run_streaming(
            &Spec {
                argv: &argv,
                cwd: &dir,
                timeout: Duration::from_secs(10),
                pass_env: &[],
                run_id: "r2",
                inherit_env: false,
                set_env: &[],
            },
            &mut |l| seen.push((l.to_owned(), started.elapsed())),
        );
        assert_eq!(e.exit_code, Some(0));
        assert_eq!(e.stdout_text(), "first\nsecond\n");
        let names: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(names, ["first", "second"]);
        // "first" was handed over before "second" was even written.
        assert!(seen[0].1 < Duration::from_millis(900), "{:?}", seen[0].1);
    }

    #[test]
    fn a_timeout_kills_the_whole_group() {
        let started = Instant::now();
        let e = sh("sleep 30 & sleep 30", Duration::from_millis(300));
        assert_eq!(e.ended, Ended::TimedOut);
        assert!(!e.complete());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn credentials_do_not_reach_the_command() {
        // SAFETY-free: the variable is set on this process only for the child to not see it.
        let argv = vec![
            "sh".into(),
            "-c".into(),
            "echo ${ANTHROPIC_API_KEY:-absent} $CONDUCTOR_RUN_ID".into(),
        ];
        let dir = std::env::temp_dir();
        let e = run(&Spec {
            argv: &argv,
            cwd: &dir,
            timeout: Duration::from_secs(10),
            pass_env: &[],
            run_id: "r9",
            inherit_env: false,
            set_env: &[],
        });
        assert_eq!(e.stdout_text().trim(), "absent r9");
        assert!(
            !inherits("ANTHROPIC_API_KEY")
                && !inherits("GITHUB_TOKEN")
                && !inherits("SSH_AUTH_SOCK")
        );
    }

    #[test]
    fn an_agent_keeps_its_credentials_but_not_its_parents_session() {
        assert!(couples_to_parent_session("CLAUDECODE"));
        assert!(couples_to_parent_session("CLAUDE_CODE_SESSION_ID"));
        assert!(couples_to_parent_session("CLAUDE_CODE_REMOTE_SESSION_ID"));
        assert!(!couples_to_parent_session("ANTHROPIC_API_KEY"));
        assert!(!couples_to_parent_session("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(!couples_to_parent_session("HOME"));
    }

    #[test]
    fn a_missing_program_is_not_run_with_a_reason() {
        let argv = vec!["definitely-not-a-program-xyz".to_string()];
        let dir = std::env::temp_dir();
        let e = run(&Spec {
            argv: &argv,
            cwd: &dir,
            timeout: Duration::from_secs(5),
            pass_env: &[],
            run_id: "r",
            inherit_env: false,
            set_env: &[],
        });
        assert_eq!(e.ended, Ended::NotRun);
        assert!(e.reason.unwrap().contains("could not start"));
    }

    #[test]
    fn an_empty_command_is_refused() {
        let dir = std::env::temp_dir();
        let e = run(&Spec {
            argv: &[],
            cwd: &dir,
            timeout: Duration::from_secs(5),
            pass_env: &[],
            run_id: "r",
            inherit_env: false,
            set_env: &[],
        });
        assert_eq!(e.ended, Ended::NotRun);
    }
}
