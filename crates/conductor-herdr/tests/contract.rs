//! Contract tests against a real herdr. They run only when `CONDUCTOR_HERDR_BIN` names a
//! herdr binary; each test starts its own named session and stops it afterwards.

use conductor_herdr::{Direction, Herdr, HerdrError, Target};
use std::process::Command;
use std::time::{Duration, Instant};

struct Session {
    bin: String,
    name: String,
}

impl Session {
    fn start(name: &str) -> Option<Session> {
        let bin = std::env::var("CONDUCTOR_HERDR_BIN").ok()?;
        let name = format!("{name}-{}", std::process::id());
        Command::new(&bin)
            .args(["--session", &name, "server"])
            .spawn()
            .expect("start herdr server");
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let ok = Command::new(&bin)
                .args(["--session", &name, "status", "--json"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok {
                return Some(Session { bin, name });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("herdr server did not come up");
    }

    fn herdr(&self) -> Herdr {
        Herdr::with(&self.bin, Target::Session(self.name.clone()))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new(&self.bin)
            .args(["--session", &self.name, "server", "stop"])
            .output();
    }
}

#[test]
fn a_run_tab_with_a_pane_that_runs_a_command_and_is_read_back() {
    let Some(s) = Session::start("conductor-contract") else {
        eprintln!("CONDUCTOR_HERDR_BIN not set; skipping");
        return;
    };
    let h = s.herdr();
    assert_eq!(h.check_protocol().unwrap(), 22);

    let dir = tempfile::tempdir().unwrap();
    let (ws, first) = h.workspace_create("conductor", dir.path()).unwrap();
    assert!(h.workspaces().unwrap().contains(&ws));
    let tab = h.tab_create(&ws, "run-01ABC", dir.path()).unwrap();
    assert_ne!(tab.tab_id, first.tab_id);

    let pane = h
        .pane_split(&tab.root_pane, Direction::Right, dir.path())
        .unwrap();
    h.pane_run(&pane, "echo conductor-$((6*7))").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut text = String::new();
    while Instant::now() < deadline {
        text = h.pane_read(&pane, 40).unwrap();
        if text.contains("conductor-42") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(text.contains("conductor-42"), "{text}");

    h.pane_close(&pane).unwrap();
    h.tab_close(&tab.tab_id).unwrap();
}

#[test]
fn conductor_refuses_to_close_what_it_did_not_open() {
    let Some(s) = Session::start("conductor-owned") else {
        eprintln!("CONDUCTOR_HERDR_BIN not set; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let theirs = s.herdr();
    let (_, their_tab) = theirs.workspace_create("someone-else", dir.path()).unwrap();

    let ours = s.herdr();
    assert!(matches!(
        ours.pane_close(&their_tab.root_pane),
        Err(HerdrError::NotOwned(_))
    ));
    assert!(matches!(
        ours.tab_close(&their_tab.tab_id),
        Err(HerdrError::NotOwned(_))
    ));
}

#[test]
fn a_server_error_comes_back_with_herdrs_code() {
    let Some(s) = Session::start("conductor-errors") else {
        eprintln!("CONDUCTOR_HERDR_BIN not set; skipping");
        return;
    };
    let h = s.herdr();
    let err = h
        .tab_create("w999", "x", std::path::Path::new("/tmp"))
        .unwrap_err();
    assert!(matches!(err, HerdrError::Server { .. }), "{err}");
    assert!(err.to_string().contains("not_found"), "{err}");
}
