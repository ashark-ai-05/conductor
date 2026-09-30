//! Read only test cases captured by Conductor's gates. Agent summaries are not input.
use crate::store::RunDir;
use conductor_checks::cargo_test::{Outcome, TestReport};
use conductor_model::{
    Verdict,
    task::{TaskChange, TestResult, TestResults},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Stage {
    stage: String,
    attempts: usize,
    gates: Vec<Gate>,
}
#[derive(Deserialize)]
struct Gate {
    gate: String,
    verdict: Verdict,
    detail: String,
    tests: Option<TestReport>,
    #[serde(default)]
    executions: Vec<Execution>,
}
#[derive(Deserialize)]
struct Execution {
    argv: Vec<String>,
    exit_code: Option<i32>,
    stdout_sha256: String,
    stderr_tail: String,
}

pub fn read(dir: &RunDir, evidence: &mut Vec<(String, String)>) -> Option<TestResults> {
    let bytes = std::fs::read(dir.gates()).ok()?;
    let stages: Vec<Stage> = match serde_json::from_slice(&bytes) {
        Ok(stages) => stages,
        Err(_) => {
            return Some(TestResults {
                cases: vec![],
                notices: vec![
                    "The saved test record could not be read. Open Evidence for available checks."
                        .into(),
                ],
            });
        }
    };
    let mut results = TestResults::default();
    let mut found = false;
    for stage in stages {
        for (index, gate) in stage.gates.into_iter().enumerate() {
            let Some(report) = gate.tests else { continue };
            found = true;
            let label = format!(
                "{} · attempt {} · check {} ({})",
                stage.stage,
                stage.attempts,
                index + 1,
                gate.gate
            );
            let mut detail = format!(
                "{label}\nGate: {}\n{}\n\nLatest parsed report from {} command execution(s). Earlier attempts and baseline cases are excluded.\n",
                gate.verdict.word(),
                gate.detail,
                gate.executions.len()
            );
            results
                .notices
                .push(format!("{label} · gate {}", gate.verdict.word()));
            if gate.executions.len() > 1 {
                results.notices.push(format!(
                    "{label} · Cases show the latest parsed report; the gate verdict includes all reruns."
                ));
            }
            if report.compiled == Some(false) || report.compile_errors > 0 {
                results.notices.push(format!(
                    "{label} · Build/load failed · {} error(s): {}",
                    report.compile_errors,
                    report.compile_messages.join(" · ")
                ));
            }
            if report.tests.is_empty() {
                results.notices.push(format!(
                    "{label} · No test cases were recorded for this check."
                ));
            }
            for (n, execution) in gate.executions.iter().enumerate() {
                detail.push_str(&format!(
                    "\nExecution {}: {}\nExit: {:?}\nStdout hash: {}\nCaptured stderr tail:\n{}\n",
                    n + 1,
                    execution.argv.join(" "),
                    execution.exit_code,
                    execution.stdout_sha256,
                    execution.stderr_tail
                ));
            }
            detail.push_str("\nFull stdout is not retained in gates.json. The parsed test message is shown when captured.\n");
            if !report.report_files.is_empty() {
                detail.push_str(&format!(
                    "\nReport files and hashes:\n{}\n",
                    report.report_files.join("\n")
                ));
            }
            if !report.compile_messages.is_empty() {
                evidence.push((
                    format!("Build/load output · {label}"),
                    format!("{}\n\n{detail}", report.compile_messages.join("\n")),
                ));
            }
            for case in report.tests {
                let outcome = match case.outcome {
                    Outcome::Passed => "Passed",
                    Outcome::Failed => "Failed",
                    Outcome::Ignored => "Ignored",
                };
                let evidence_index = evidence.len();
                evidence.push((
                    format!("{outcome} · {}", case.name),
                    format!(
                        "{}\n\n{}\n\n{detail}",
                        case.name,
                        case.message
                            .as_deref()
                            .unwrap_or("No per-test message was captured.")
                    ),
                ));
                results.cases.push(TestResult {
                    id: format!("{}/{}/{}/{}", stage.stage, stage.attempts, index, case.name),
                    name: case.name,
                    outcome: outcome.into(),
                    stage: stage.stage.clone(),
                    attempt: stage.attempts,
                    evidence: evidence_index,
                });
            }
        }
    }
    results.cases.sort_by_key(|c| match c.outcome.as_str() {
        "Failed" => 0,
        "Ignored" => 1,
        _ => 2,
    });
    found.then_some(results)
}

/// Read only patches named by completed gate records. Never pair an in-flight retry
/// with the previous attempt's test results. Limits keep a generated patch bounded.
pub fn changes(dir: &RunDir) -> Vec<TaskChange> {
    use std::io::Read;
    let stages: Vec<Stage> = std::fs::read(dir.gates())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for stage in stages {
        if stage.attempts == 0 || stage.stage.contains(['/', '\\']) || stage.stage == ".." {
            continue;
        }
        let Ok(file) = std::fs::File::open(dir.attempt_diff(&stage.stage, stage.attempts)) else {
            continue;
        };
        const PATCH_LIMIT: usize = 512 * 1024;
        let mut bytes = Vec::new();
        if file
            .take((PATCH_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        let truncated = bytes.len() > PATCH_LIMIT;
        bytes.truncate(PATCH_LIMIT);
        let text = String::from_utf8_lossy(&bytes);
        let mut current: Option<TaskChange> = None;
        for line in text.lines() {
            if let Some(header) = line.strip_prefix("diff --git ") {
                if let Some(file) = current.take() {
                    out.push(file);
                }
                if out.len() >= 100 {
                    if let Some(last) = out.last_mut() {
                        last.patch.push_str("\n[Preview limited to 100 files. More changes are saved in the run's attempts directory.]\n");
                    }
                    return out;
                }
                current = Some(TaskChange {
                    stage: stage.stage.clone(),
                    attempt: stage.attempts,
                    path: header
                        .split_once(" b/")
                        .map(|(_, b)| b)
                        .unwrap_or(header)
                        .to_owned(),
                    patch: String::new(),
                });
            }
            if let Some(file) = &mut current {
                file.patch.push_str(line);
                file.patch.push('\n');
            }
        }
        if let Some(mut file) = current {
            if truncated {
                file.patch.push_str(&format!(
                    "\n[Preview limited to 512 KiB. Complete saved patch: {}]\n",
                    dir.attempt_diff(&stage.stage, stage.attempts).display()
                ));
            }
            out.push(file);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn latest_cases_link_to_engine_evidence_without_counting_baselines_or_reruns_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RunDir::create(tmp.path(), "run").unwrap();
        let report = json!({"compiled":true,"compile_errors":0,"tests":[
            {"name":"ok","outcome":"passed"},
            {"name":"broken","outcome":"failed","message":"assertion: expected 200, got 404"}
        ]});
        let exec = json!({"argv":["cargo","test"],"exit_code":101,"stdout_sha256":"sha256:output","stderr_tail":"captured error"});
        dir.write_json(&dir.gates(), &json!([{"stage":"test","attempts":2,
            "before":[{"tests":report}],
            "gates":[{"gate":"command_assert","verdict":"flaky","detail":"mixed reruns","tests":report,"executions":[exec,exec]}]
        }])).unwrap();
        let mut evidence = vec![("Existing check".into(), "Keep".into())];
        let result = read(&dir, &mut evidence).unwrap();
        assert_eq!(result.cases.len(), 2);
        assert_eq!(result.cases[0].name, "broken");
        assert_eq!(result.cases[0].attempt, 2);
        assert!(result.notices.iter().any(|n| n.contains("flaky")));
        let detail = &evidence[result.cases[0].evidence].1;
        assert!(detail.contains("expected 200, got 404"));
        assert!(detail.contains("cargo test") && detail.contains("sha256:output"));
        assert!(detail.contains("Full stdout is not retained"));
    }

    #[test]
    fn only_the_patch_named_by_a_recorded_attempt_is_displayed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RunDir::create(tmp.path(), "run").unwrap();
        std::fs::create_dir_all(dir.root.join("attempts")).unwrap();
        std::fs::write(
            dir.attempt_diff("fix", 1),
            "diff --git a/one.rs b/one.rs\n-old\n+first\n",
        )
        .unwrap();
        std::fs::write(
            dir.attempt_diff("fix", 2),
            "diff --git a/one.rs b/one.rs\n-old\n+second\n",
        )
        .unwrap();
        assert!(changes(&dir).is_empty());
        dir.write_json(
            &dir.gates(),
            &json!([{"stage":"fix","attempts":1,"gates":[]}]),
        )
        .unwrap();
        let shown = changes(&dir);
        assert_eq!(shown[0].attempt, 1);
        assert!(shown[0].patch.contains("+first"));
        assert!(!shown[0].patch.contains("+second"));
        dir.write_json(
            &dir.gates(),
            &json!([{"stage":"fix","attempts":2,"gates":[]}]),
        )
        .unwrap();
        assert!(changes(&dir)[0].patch.contains("+second"));
        dir.write_json(
            &dir.gates(),
            &json!([{"stage":"../outside","attempts":1,"gates":[]}]),
        )
        .unwrap();
        assert!(changes(&dir).is_empty());
    }

    #[test]
    fn oversized_patch_previews_keep_utf8_and_disclose_omissions() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RunDir::create(tmp.path(), "run").unwrap();
        std::fs::create_dir_all(dir.root.join("attempts")).unwrap();
        dir.write_json(
            &dir.gates(),
            &json!([{"stage":"fix","attempts":1,"gates":[]}]),
        )
        .unwrap();
        let mut patch = "diff --git a/one.rs b/one.rs\n+".to_owned();
        patch.push_str(&"x".repeat(512 * 1024 - patch.len() - 1));
        patch.push('é');
        std::fs::write(dir.attempt_diff("fix", 1), patch).unwrap();
        let shown = changes(&dir);
        assert_eq!(shown.len(), 1);
        assert!(shown[0].patch.contains("Preview limited to 512 KiB"));
        assert!(shown[0].patch.contains("Complete saved patch:"));
        let files: String = (0..102)
            .map(|i| format!("diff --git a/{i}.rs b/{i}.rs\n+change\n"))
            .collect();
        std::fs::write(dir.attempt_diff("fix", 1), files).unwrap();
        let shown = changes(&dir);
        assert_eq!(shown.len(), 100);
        assert!(
            shown
                .last()
                .unwrap()
                .patch
                .contains("Preview limited to 100 files")
        );
    }

    #[test]
    fn compile_failures_and_missing_reports_are_never_successful_test_suites() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RunDir::create(tmp.path(), "run").unwrap();
        let mut evidence = vec![];
        assert!(read(&dir, &mut evidence).is_none());
        dir.write_json(&dir.gates(), &json!([{"stage":"test","attempts":1,"gates":[
            {"gate":"command_assert","verdict":"failed","detail":"did not compile","tests":{"compiled":false,"compile_errors":1,"compile_messages":["missing type"],"tests":[]}}
        ]}])).unwrap();
        let result = read(&dir, &mut evidence).unwrap();
        assert!(result.cases.is_empty());
        assert!(
            result
                .notices
                .iter()
                .any(|n| n.contains("Build/load failed"))
        );
        assert!(evidence[0].1.contains("missing type"));
        std::fs::write(dir.gates(), b"malformed").unwrap();
        assert!(read(&dir, &mut evidence).unwrap().notices[0].contains("could not be read"));
    }
}
