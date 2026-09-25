//! The SDLC mock (examples/sdlc-mock) run for real, once per ticket: a scripted agent
//! applies the fix, conductor deploys the Spring Boot service and checks that ticket's
//! acceptance criteria, the evidence lands in the ticket, the run waits for the PO, and
//! `conductor approve` finishes it. Needs Java and Maven; skipped without them unless
//! CONDUCTOR_LANG_TESTS=1.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn have(program: &str, args: &[&str]) -> bool {
    let ok = Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        assert!(
            std::env::var("CONDUCTOR_LANG_TESTS").as_deref() != Ok("1"),
            "`{program}` is needed for this test"
        );
        eprintln!("skipped: `{program} {}` is not available", args.join(" "));
    }
    ok
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

/// A ticket, the one-line fix a scripted agent applies for it, a line its diff must
/// contain, and a line its application log must show.
struct Ticket {
    id: &'static str,
    fix: &'static str,
    diff_line: &'static str,
    log_line: &'static str,
    /// The criterion the bug breaks, as the before block shows it.
    fails_before: &'static str,
}

const JAVA: &str = "src/main/java/com/example/accounts/LedgerService.java";

const TICKETS: [Ticket; 3] = [
    Ticket {
        id: "BUG-101",
        fix: "s/BigDecimal total = null;/BigDecimal total = BigDecimal.ZERO;/; s/total = total == null ? p : total.add(p);/total = total.add(p);/",
        diff_line: "+        BigDecimal total = BigDecimal.ZERO;",
        log_line: "balance for ACC-2: 0.00",
        fails_before: "FAIL AC2:",
    },
    Ticket {
        id: "BUG-102",
        fix: "s/ledger.get(accountId)/ledger.get(accountId.toUpperCase(java.util.Locale.ROOT))/",
        diff_line: "+        List<BigDecimal> postings = ledger.get(accountId.toUpperCase(java.util.Locale.ROOT));",
        log_line: "balance for acc-1: 150.00",
        fails_before: "FAIL AC1:",
    },
    Ticket {
        id: "BUG-103",
        fix: "s/RoundingMode.HALF_EVEN/RoundingMode.HALF_UP/",
        diff_line: "+        BigDecimal balance = total.setScale(2, RoundingMode.HALF_UP);",
        log_line: "balance for ACC-3: 10.01",
        fails_before: "FAIL AC1:",
    },
];

fn title_of(id: &str) -> &'static str {
    match id {
        "BUG-101" => "BUG-101: Balance request fails for an account with no postings",
        "BUG-102" => "BUG-102: Balance request fails when the account id is typed in lower case",
        _ => "BUG-103: Balance is a cent short of the ledger for a half-cent posting",
    }
}

/// The three tickets run one after another: the deploy binds one port.
#[test]
fn each_bug_fix_is_deployed_checked_evidenced_and_approved() {
    if !have("java", &["-version"]) || !have("mvn", &["-v"]) {
        return;
    }
    for t in &TICKETS {
        one_ticket(t);
    }
}

fn one_ticket(t: &Ticket) {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/sdlc-mock"),
        repo,
    );
    // The scripted agent stands in for Claude: it applies the ticket's one-line fix.
    let wf = repo.join(".conductor/workflows/bugfix.yaml");
    let mut yaml = std::fs::read_to_string(&wf).unwrap();
    let from = yaml.find("    agent:\n      kind: claude").unwrap();
    let to = yaml
        .find("    prompt_file: .conductor/prompts/fix.md\n")
        .unwrap();
    let fix = format!("sed -i.bak '{}' {JAVA} && rm -f {JAVA}.bak", t.fix);
    yaml.replace_range(
        from..to,
        &format!("    agent: {{ kind: script, command: [\"sh\", \"-c\", {fix:?}] }}\n"),
    );
    std::fs::write(&wf, yaml).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "accounts with three bugs"]);

    let ticket_path = format!("tickets/{}.md", t.id);
    let mut run = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/bugfix.yaml",
            "--spec",
            &ticket_path,
            "--executor",
            "headless",
        ])
        .current_dir(repo)
        .env("CONDUCTOR_HOME", repo.join(".home"))
        .env_remove("CONDUCTOR_OTLP_ENDPOINT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Wait for the run to pause on the PO, then decide.
    let started = Instant::now();
    let waiting = loop {
        let ids = conductor_engine::list_runs_any(repo);
        if let Some(id) = ids.first()
            && let Some(l) = conductor_engine::live::read(repo, id)
            && let Some(w) = l.waiting
        {
            break (id.clone(), w);
        }
        assert!(
            started.elapsed() < Duration::from_secs(900),
            "{}: the run never waited for the PO",
            t.id
        );
        if let Some(status) = run.try_wait().unwrap() {
            let out = run.wait_with_output().unwrap();
            panic!(
                "{}: the run ended ({status}) before waiting:\n{}{}",
                t.id,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let (run_id, w) = waiting;
    assert_eq!((w.stage.as_str(), w.who.as_str()), ("review", "PO"));
    assert!(w.question.contains("Approve only if"), "{}", w.question);
    // The one screen to decide from: the ticket's criteria, the change, each check before
    // and after with its result lines, and the cost.
    let review = conductor_engine::review::read(repo, &run_id).expect("review.json");
    assert_eq!(review.title.as_str(), title_of(t.id));
    assert_eq!(review.criteria.len(), 4, "{:?}", review.criteria);
    assert!(review.criteria[0].starts_with("AC1:"));
    assert_eq!(review.change.files, vec![JAVA]);
    let deploy = review
        .evidence
        .iter()
        .find(|e| e.claim.contains("deploy-and-test"))
        .expect("the deploy check");
    assert_eq!(deploy.before, Some(conductor_model::Verdict::Failed));
    assert_eq!(deploy.after, conductor_model::Verdict::Passed);
    assert!(
        deploy.lines.iter().all(|l| l.starts_with("PASS AC")) && deploy.lines.len() == 3,
        "{:?}",
        deploy.lines
    );
    assert_eq!(review.cost.tries, 1);

    let approve = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "approve",
            &run_id,
            "--by",
            "PO",
            "-m",
            "AC1-3 shown against the deployed build",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        approve.status.success(),
        "{}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let out = run.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{}: {text}", t.id);
    assert!(text.contains("PASSED"), "{text}");
    assert!(text.contains("PO approved: AC1-3 shown"), "{text}");
    assert!(
        text.contains("PO approved the work so far (review)"),
        "{text}"
    );

    // The ticket, on the run's branch, carries the evidence and the decision.
    let branch = format!("conductor/{run_id}");
    let ticket = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{branch}:{ticket_path}")])
        .output()
        .unwrap();
    let ticket = String::from_utf8_lossy(&ticket.stdout);
    assert!(ticket.contains("## Evidence"), "{ticket}");
    // Before the fix the ticket's own criterion fails; after it every one passes.
    let (before, after) = ticket
        .split_once("**After the fix**")
        .unwrap_or_else(|| panic!("{}: no before/after in {ticket}", t.id));
    assert!(before.contains("**Before the fix**"), "{}: {ticket}", t.id);
    assert!(before.contains(t.fails_before), "{}: {before}", t.id);
    for ac in ["PASS AC1:", "PASS AC2:", "PASS AC3:"] {
        assert!(after.contains(ac), "{}: {after}", t.id);
    }
    assert!(!after.contains("FAIL AC"), "{after}");
    assert!(after.contains("- **change**: 1 file(s)"), "{after}");
    assert!(after.contains(t.diff_line), "{}: {after}", t.id);
    assert!(!after.contains("sun.misc.Unsafe"), "{after}");
    assert!(ticket.contains("**logs/app.log**"), "{ticket}");
    assert!(ticket.contains(t.log_line), "{}: {ticket}", t.id);
    assert!(
        ticket.contains("**decision** passed: approved by PO: AC1-3 shown"),
        "{ticket}"
    );
    // A stage's diff is the fix, not the evidence conductor wrote.
    let diff = std::fs::read_to_string(
        conductor_engine::store::RunDir::for_run(repo, &run_id).attempt_diff("fix", 1),
    )
    .unwrap();
    assert!(diff.contains(t.diff_line), "{}: {diff}", t.id);
}
