//! `conductor serve`: the run record in a browser.
//!
//! A small local server over `.conductor/runs/`: a JSON API and one embedded page, no
//! build step and nothing written. It is for the people who never open a terminal, the
//! reviewer, the auditor, the lead, and for watching a run as it happens: the page polls
//! while a run is live. The record on disk stays the only truth; this reads it.

pub mod api;
pub mod timeline;

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tiny_http::{Header, Method, Request, Response, StatusCode};

const INDEX: &str = include_str!("../ui/index.html");
const APP_JS: &str = include_str!("../ui/app.js");
const APP_CSS: &str = include_str!("../ui/app.css");

pub struct Server {
    inner: tiny_http::Server,
    repo: Arc<PathBuf>,
}

impl Server {
    /// Binds `addr` (`127.0.0.1:0` picks a free port) over `repo`'s runs.
    pub fn bind(repo: &Path, addr: &str) -> io::Result<Self> {
        let inner = tiny_http::Server::http(addr).map_err(io::Error::other)?;
        Ok(Server {
            inner,
            repo: Arc::new(repo.to_path_buf()),
        })
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        match self.inner.server_addr() {
            tiny_http::ListenAddr::IP(a) => Some(a),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => None,
        }
    }

    /// Serves until the process ends. Each request gets its own thread; they are short.
    pub fn serve_forever(&self) {
        for req in self.inner.incoming_requests() {
            let repo = Arc::clone(&self.repo);
            std::thread::spawn(move || {
                let _ = handle(&repo, req);
            });
        }
    }
}

type Reply = Response<io::Cursor<Vec<u8>>>;

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("static header")
}

fn text(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Reply {
    Response::from_data(body.into())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
}

fn json<T: serde::Serialize>(v: Option<T>) -> Reply {
    match v.map(|v| serde_json::to_vec(&v)) {
        Some(Ok(b)) => text(200, "application/json", b),
        Some(Err(e)) => text(500, "text/plain; charset=utf-8", e.to_string()),
        None => text(404, "text/plain; charset=utf-8", "not found"),
    }
}

/// A run id is 13 base-36 digits; anything else is not a path conductor wrote.
fn run_id_ok(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn query(url: &str, key: &str) -> Option<String> {
    let (_, q) = url.split_once('?')?;
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_owned())
}

/// The response for one request. Split out so tests can drive it without sockets.
pub fn respond(repo: &Path, method: &Method, url: &str, body: &str) -> Reply {
    let path = url.split('?').next().unwrap_or(url);
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    // The one write: a person's decision on a stage the run is waiting for.
    if *method == Method::Post {
        return match parts.as_slice() {
            ["api", "runs", id, "decide"] if run_id_ok(id) => decide(repo, id, body),
            _ => text(405, "text/plain; charset=utf-8", "read only"),
        };
    }
    if *method != Method::Get && *method != Method::Head {
        return text(405, "text/plain; charset=utf-8", "read only");
    }
    match parts.as_slice() {
        ["app.js"] => text(200, "text/javascript; charset=utf-8", APP_JS),
        ["app.css"] => text(200, "text/css; charset=utf-8", APP_CSS),
        ["api", "runs"] => json(Some(api::runs(repo))),
        ["api", "stats"] => json(Some(api::stats(repo))),
        ["api", "runs", id] if run_id_ok(id) => json(api::run(repo, id)),
        ["api", "runs", id, "tail"] if run_id_ok(id) => {
            let since = query(url, "since")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            json(api::tail(repo, id, since))
        }
        ["api", "runs", id, "diff", stage, n] if run_id_ok(id) => {
            match n.parse().ok().and_then(|n| api::diff(repo, id, stage, n)) {
                Some(d) => text(200, "text/x-diff; charset=utf-8", d),
                None => text(404, "text/plain; charset=utf-8", "not found"),
            }
        }
        // The page routes by hash, so any path without an extension is the page.
        [first] if !first.contains('.') => text(200, "text/html; charset=utf-8", INDEX),
        _ => text(404, "text/plain; charset=utf-8", "not found"),
    }
}

#[derive(serde::Deserialize)]
struct DecideBody {
    stage: String,
    approved: bool,
    #[serde(default)]
    by: String,
    #[serde(default)]
    note: String,
}

fn decide(repo: &Path, id: &str, body: &str) -> Reply {
    let b: DecideBody = match serde_json::from_str(body) {
        Ok(b) => b,
        Err(e) => return text(400, "text/plain; charset=utf-8", e.to_string()),
    };
    if b.stage.contains(['/', '\\', '.']) {
        return text(400, "text/plain; charset=utf-8", "bad stage");
    }
    let dir = conductor_engine::store::RunDir::for_run(repo, id);
    if !dir.events().is_file() {
        return text(404, "text/plain; charset=utf-8", "not found");
    }
    let d = conductor_engine::decision::Decision {
        approved: b.approved,
        by: if b.by.trim().is_empty() {
            "someone".into()
        } else {
            b.by.trim().to_owned()
        },
        note: b.note,
        at: conductor_engine::store::now(),
    };
    match conductor_engine::decision::write(&dir, &b.stage, &d) {
        Ok(()) => json(Some(&d)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            text(409, "text/plain; charset=utf-8", e.to_string())
        }
        Err(e) => text(500, "text/plain; charset=utf-8", e.to_string()),
    }
}

fn handle(repo: &Path, mut req: Request) -> io::Result<()> {
    let mut body = String::new();
    if *req.method() == Method::Post {
        use std::io::Read as _;
        let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
    }
    let resp = respond(repo, req.method(), req.url(), &body);
    req.respond(resp)
}

/// Opens `url` in the person's browser, if there is a way to.
pub fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
