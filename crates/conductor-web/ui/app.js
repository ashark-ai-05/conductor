/* conductor's web page: runs, one run as two lanes, and stats. Plain JS, no build. */
"use strict";

const $ = (sel, el = document) => el.querySelector(sel);
const h = (tag, attrs = {}, ...kids) => {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") el.className = v;
    else if (k === "html") el.innerHTML = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined) el.setAttribute(k, v);
  }
  for (const k of kids.flat()) {
    if (k === null || k === undefined) continue;
    el.append(k.nodeType ? k : document.createTextNode(String(k)));
  }
  return el;
};
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const money = (x) => "$" + (x || 0).toFixed(2);
const tokens = (n) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(0) + "k" : String(n || 0));
const dur = (s) => {
  if (s === null || s === undefined) return "—";
  const m = Math.floor(s / 60), sec = Math.floor(s % 60);
  return m >= 60 ? `${Math.floor(m / 60)}h${String(m % 60).padStart(2, "0")}m` : `${m}m${String(sec).padStart(2, "0")}s`;
};
const clock = (ms) => {
  const s = Math.floor(ms / 1000);
  return `+${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
};
const ago = (iso) => {
  const d = (Date.now() - Date.parse(iso)) / 1000;
  if (!isFinite(d)) return "";
  if (d < 90) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  return `${Math.round(d / 86400)} d ago`;
};
const GLYPH = { passed: "✓", failed: "✗", running: "●", pending: "○", unwitnessed: "?", flaky: "~", blocked: "■", overridden: "≈" };
const glyph = (v) => h("span", { class: `glyph g-${v}` }, GLYPH[v] || "○");
const pill = (v) => h("span", { class: `pill v-${v}` }, v);

async function get(url) {
  const r = await fetch(url, { cache: "no-store" });
  if (!r.ok) throw new Error(`${r.status} ${url}`);
  return r.headers.get("content-type")?.includes("json") ? r.json() : r.text();
}

// ── routing ────────────────────────────────────────────────────────────────
let stop = () => {};
function route() {
  stop();
  stop = () => {};
  const hash = location.hash.replace(/^#\/?/, "");
  const main = $("#main");
  main.replaceChildren();
  for (const a of document.querySelectorAll("[data-nav]")) a.classList.toggle("on", a.dataset.nav === (hash.startsWith("stats") ? "stats" : "runs"));
  if (hash.startsWith("run/")) runPage(main, hash.slice(4).split("/")[0]);
  else if (hash.startsWith("stats")) statsPage(main);
  else runsPage(main);
}
window.addEventListener("hashchange", route);
window.addEventListener("DOMContentLoaded", route);

// ── runs ───────────────────────────────────────────────────────────────────
function tiles(runs) {
  const done = runs.filter((r) => !r.running);
  const passed = done.filter((r) => r.verdict === "passed").length;
  const cost = runs.reduce((a, r) => a + r.cost_usd, 0);
  const attempts = runs.reduce((a, r) => a + r.stages.reduce((b, s) => b + s.attempts, 0), 0);
  const catches = runs.reduce((a, r) => a + r.catches, 0);
  const perPass = passed ? done.filter((r) => r.verdict === "passed").reduce((a, r) => a + r.cost_usd, 0) / passed : 0;
  const costs = [...runs].reverse().map((r) => r.cost_usd);
  return h("div", { class: "tiles" },
    tile(`${passed} of ${done.length}`, "runs passed", spark(done.slice().reverse().map((r) => (r.verdict === "passed" ? 1 : 0)), "pass")),
    tile(attempts ? `${Math.round((100 * catches) / attempts)}%` : "—", "attempts caught by a check: the agent said done, a check said no"),
    tile(money(cost), `spent · ${money(perPass)} per verified change`, spark(costs, "accent")),
    tile(runs.filter((r) => r.running).length, "running now"),
  );
}
const tile = (v, l, sp) => h("div", { class: "tile" }, h("div", { class: "v" }, v), h("div", { class: "l" }, l), sp || null);
function spark(vals, color) {
  if (vals.length < 2) return null;
  const W = 160, H = 22, max = Math.max(...vals, 1e-9);
  const pts = vals.map((v, i) => `${(i / (vals.length - 1)) * W},${H - 2 - (v / max) * (H - 4)}`).join(" ");
  return h("div", { class: "spark", html: `<svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" aria-hidden="true"><polyline points="${pts}" fill="none" stroke="var(--${color})" stroke-width="2" stroke-linejoin="round"/></svg>` });
}
function pipe(stages) {
  return h("span", { class: "pipe" }, stages.flatMap((s, i) => [
    h("span", { class: "st", title: `${s.name}: ${s.status}, ${s.attempts} tr${s.attempts === 1 ? "y" : "ies"}` }, glyph(s.status), s.name, s.attempts > 1 ? h("span", { class: "id" }, `×${s.attempts}`) : null),
    i < stages.length - 1 ? "→" : null,
  ]));
}
async function runsPage(main) {
  let runs;
  try { runs = await get("/api/runs"); } catch (e) { main.append(h("div", { class: "empty" }, "could not load runs: " + e.message)); return; }
  if (!runs.length) {
    main.append(h("div", { class: "empty" }, "No runs recorded in this repository yet. Start one with ", h("code", {}, "conductor run <workflow> --spec <file>"), "."));
    return;
  }
  main.append(tiles(runs));
  const rows = runs.map((r) => h("tr", { class: "row", onclick: () => (location.hash = `#/run/${r.run_id}`) },
    h("td", {}, r.running ? h("span", {}, h("span", { class: "live-dot" }), pill("running")) : pill(r.verdict)),
    h("td", {}, h("div", { class: "work" }, r.work), h("div", { class: "id" }, `${r.run_id} · ${r.kind}`)),
    h("td", {}, pipe(r.stages)),
    h("td", { class: "num" }, r.checks ? `${r.checks_passed}/${r.checks}` : "—"),
    h("td", { class: "num" }, r.catches || "—"),
    h("td", { class: "num" }, r.survivors || "—"),
    h("td", { class: "num" }, money(r.cost_usd)),
    h("td", { class: "num" }, r.running ? "…" : dur(r.duration_s)),
    h("td", { class: "id" }, ago(r.started_at)),
  ));
  main.append(h("table", { class: "runs" },
    h("thead", {}, h("tr", {}, h("th", {}, "verdict"), h("th", {}, "work"), h("th", {}, "stages"), h("th", { class: "num" }, "checks"), h("th", { class: "num" }, "catches"), h("th", { class: "num" }, "survivors"), h("th", { class: "num" }, "cost"), h("th", { class: "num" }, "time"), h("th", {}, "started"))),
    h("tbody", {}, rows)));
  if (runs.some((r) => r.running)) { const t = setInterval(() => route(), 3000); stop = () => clearInterval(t); }
}

// ── run ────────────────────────────────────────────────────────────────────
function story(d) {
  const s = d.summary, t = d.timeline, r = d.receipt;
  const parts = [];
  const byStage = new Map();
  for (const a of t.attempts) byStage.set(a.stage, [...(byStage.get(a.stage) || []), a]);
  for (const [stage, atts] of byStage) {
    const last = atts[atts.length - 1];
    const who = last.agent;
    let line = `${stage}: ${who}`;
    if (last.outcome === "passed") line += atts.length === 1 ? " passed first try" : ` passed on try ${atts.length}`;
    else if (last.outcome === "running") line += atts.length === 1 ? " is working" : ` is on try ${atts.length}`;
    else if (last.outcome === "check_failed") line += ` was stopped by ${last.caught_by} after ${atts.length} tr${atts.length === 1 ? "y" : "ies"}`;
    else line += ` ${last.outcome.replace("_", " ")}`;
    const caught = atts.filter((a) => a.caught_by).map((a) => `try ${a.attempt} by ${a.caught_by}`);
    if (caught.length && last.outcome === "passed") line += ` (caught ${caught.join(", ")})`;
    parts.push(line);
  }
  if (r && r.survivors.length) parts.push(`${r.survivors.length} injected bug${r.survivors.length === 1 ? "" : "s"} survived: look at ${r.survivors.slice(0, 2).map((x) => x.at).join(", ")} first.`);
  else if (r && r.checks.some((c) => c.claim.toLowerCase().includes("injected"))) parts.push("Every injected bug was caught.");
  return parts.join(". ").replace(/\.\./g, ".") + (parts.length && !parts[parts.length - 1].endsWith(".") ? "." : "");
}

async function runPage(main, id) {
  let d;
  try { d = await get(`/api/runs/${id}`); } catch (e) { main.append(h("div", { class: "empty" }, `run ${id}: ${e.message}`)); return; }
  const state = { d, cursor: Infinity, playing: false, timer: null, follow: d.summary.running, diffsOpen: new Set(), foldActions: true };
  const head = h("div");
  const controls = h("div", { class: "controls" });
  const lanes = h("div");
  const side = h("div");
  main.append(head, h("div", { class: "layout" }, h("div", {}, h("div", { class: "panel" }, controls, h("div", { class: "lanes-head" }, h("div", {}, "the agent did"), h("div"), h("div", { class: "r" }, "conductor witnessed")), lanes, legend())), side));

  function render() {
    const s = state.d.summary;
    head.replaceChildren(
      h("div", { class: "run-head" }, h("h1", {}, s.work), h("span", { class: "meta mono" }, `${s.run_id} · ${s.kind}`), s.running ? h("span", { class: "meta" }, h("span", { class: "live-dot" }), "live") : null, h("a", { href: "#/", class: "meta" }, "← all runs")),
      banner(state.d));
    renderLanes();
    renderSide();
  }
  function banner(d) {
    const s = d.summary, v = s.running ? "running" : s.verdict;
    return h("div", { class: `banner ${v}` },
      h("span", { class: `word v-${v}` }, s.running ? "running" : s.verdict),
      kv(money(d.timeline.cost_usd), "spent"), kv(tokens(d.timeline.tokens), "tokens"),
      kv(s.running ? clock(Date.now() - Date.parse(s.started_at)).slice(1) : dur(s.duration_s), s.running ? "elapsed" : "wall clock"),
      kv(String(d.timeline.attempts.length), "attempts"), kv(String(d.timeline.catches), "caught by a check"),
      d.receipt ? kv(`${s.checks_passed} of ${s.checks}`, "checks passed") : null,
      h("div", { class: "story" }, story(d)),
      d.live && d.live.note && s.running ? h("div", { class: "story", style: "color:var(--warn)" }, d.live.note) : null);
  }
  const kv = (v, l) => h("span", { class: "kv" }, h("b", {}, v), h("span", {}, l));

  function renderControls() {
    const t = state.d.timeline, n = t.entries.length, max = n ? t.entries[n - 1].seq : 0;
    const cur = Math.min(state.cursor, max);
    const at = t.entries.find((e) => e.seq === cur) || t.entries[n - 1];
    controls.replaceChildren(
      h("button", { onclick: () => togglePlay() }, state.playing ? "❚❚ pause" : "▶ replay"),
      h("button", { onclick: () => { state.cursor = 0; state.playing = false; renderLanes(); renderControls(); } }, "⇤"),
      h("input", { type: "range", min: 0, max, value: cur, oninput: (ev) => { state.cursor = +ev.target.value; state.playing = false; state.follow = false; renderLanes(); renderControls(); } }),
      h("button", { onclick: () => { state.cursor = Infinity; state.playing = false; state.follow = state.d.summary.running; renderLanes(); renderControls(); } }, "⇥"),
      h("span", { class: "clock" }, at ? `${clock(at.t_ms)} · event ${cur} of ${max}` : ""),
      h("button", { class: state.foldActions ? "on" : "", onclick: () => { state.foldActions = !state.foldActions; renderLanes(); renderControls(); } }, state.foldActions ? "actions folded" : "every action"),
    );
  }
  function togglePlay() {
    state.playing = !state.playing;
    if (state.playing) {
      state.follow = false;
      const t = state.d.timeline, max = t.entries.length ? t.entries[t.entries.length - 1].seq : 0;
      if (state.cursor >= max) state.cursor = 0;
      state.timer = setInterval(() => {
        state.cursor += 1;
        if (state.cursor >= max) { state.cursor = Infinity; state.playing = false; clearInterval(state.timer); }
        renderLanes(); renderControls();
      }, 90);
    } else clearInterval(state.timer);
    renderControls();
  }

  function renderLanes() {
    const t = state.d.timeline;
    const frag = document.createDocumentFragment();
    const cursor = state.cursor;
    const bands = new Map();
    for (const a of t.attempts) bands.set(`${a.stage}-${a.attempt}`, a);
    let currentBand = null, lastBandKey = null;
    let pendingActions = [];
    const flush = () => {
      if (!pendingActions.length) return;
      if (state.foldActions && pendingActions.length > 6) {
        const shown = pendingActions.slice(-3);
        const hidden = pendingActions.slice(0, -3);
        const holder = h("div");
        const fold = h("div", { class: "lane-row" }, h("div", { class: "folded", onclick: () => { holder.replaceChildren(...hidden.map(row)); } }, `… ${hidden.length} more actions (click to show)`), h("div", { class: "spine" }), h("div"));
        holder.append(fold);
        frag.append(holder, ...shown.map(row));
      } else frag.append(...pendingActions.map(row));
      pendingActions = [];
    };
    for (const e of t.entries) {
      if (e.seq > cursor) break;
      const key = e.stage && e.attempt ? `${e.stage}-${e.attempt}` : null;
      if (key && key !== lastBandKey && bands.has(key)) { flush(); currentBand = bands.get(key); lastBandKey = key; frag.append(bandEl(currentBand, cursor)); }
      if (e.kind === "action") { pendingActions.push(e); continue; }
      if (e.kind === "refused" || e.kind === "asked") { flush(); frag.append(row(e)); continue; }
      flush();
      frag.append(row(e));
    }
    flush();
    lanes.replaceChildren(frag);
    if (state.follow) lanes.lastElementChild?.scrollIntoView({ block: "nearest" });
  }
  function bandEl(a, cursor) {
    const ended = a.end_ms !== undefined && a.end_ms !== null && a.last_seq <= cursor;
    const out = a.last_seq > cursor && a.outcome !== "running" ? "running" : a.outcome;
    const key = `${a.stage}-${a.attempt}`;
    const hasDiff = state.d.diffs.some(([s, n]) => s === a.stage && n === a.attempt);
    const el = h("div", { class: "band" },
      h("span", { class: "stage" }, a.stage), h("span", { class: "how" }, `try ${a.attempt} · ${a.how} · ${a.agent}`),
      h("span", { class: "stat" }, `${a.actions} action${a.actions === 1 ? "" : "s"}`),
      a.cost_usd !== undefined ? h("span", { class: "stat" }, `${money(a.cost_usd)} · ${tokens(a.tokens)} tokens`) : null,
      ended ? h("span", { class: "stat" }, dur((a.end_ms - a.start_ms) / 1000)) : null,
      hasDiff ? h("button", { class: state.diffsOpen.has(key) ? "on" : "", onclick: () => toggleDiff(key, a) }, "diff") : null,
      h("span", { class: `out ${out}` }, out === "check_failed" ? `caught by ${a.caught_by}` : out.replace("_", " ")));
    const wrap = h("div", {}, el);
    if (state.diffsOpen.has(key)) wrap.append(diffEl(key));
    return wrap;
  }
  const diffCache = new Map();
  async function toggleDiff(key, a) {
    if (state.diffsOpen.has(key)) state.diffsOpen.delete(key);
    else { state.diffsOpen.add(key); if (!diffCache.has(key)) diffCache.set(key, await get(`/api/runs/${id}/diff/${a.stage}/${a.attempt}`)); }
    renderLanes();
  }
  function diffEl(key) {
    const text = diffCache.get(key);
    if (text === undefined) return h("div", { class: "diff" }, h("pre", {}, "loading…"));
    if (!text.trim()) return h("div", { class: "diff" }, h("div", { class: "file" }, "no changes"));
    const box = h("div", { class: "diff" });
    let pre = null;
    for (const line of text.split("\n")) {
      if (line.startsWith("diff --git")) { const m = line.match(/ b\/(.*)$/); box.append(h("div", { class: "file" }, m ? m[1] : line)); pre = h("pre"); box.append(pre); continue; }
      if (!pre || /^(index |--- |\+\+\+ |new file|deleted file|similarity|rename )/.test(line)) continue;
      const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : line.startsWith("@@") ? "hunk" : "ctx";
      pre.append(h("span", { class: `ln ${cls}` }, line + "\n"));
    }
    return box;
  }
  function row(e) {
    const lane = e.lane;
    const cls = ["lane-row", lane, e.kind === "check" ? (e.verdict === "passed" ? "check-passed" : e.verdict === "failed" ? "check-failed" : "check-other") : e.kind];
    const entry = h("div", { class: `entry ${lane} ${e.kind} src-${e.source} ${e.kind === "check" ? (e.verdict === "passed" ? "passed" : e.verdict === "failed" ? "failed" : "other") : ""}` }, h("span", { class: "t" }, clock(e.t_ms)), h("span", { class: "x" }, ...content(e)));
    return h("div", { class: cls.join(" ") }, lane === "agent" ? entry : h("div"), h("div", { class: "spine" }, h("i")), lane === "conductor" ? entry : h("div"));
  }
  function content(e) {
    if (e.kind === "action") { const [tool, ...rest] = e.text.split(" "); return [h("span", { class: "tool" }, tool), rest.join(" ")]; }
    if (e.kind === "check") return [h("span", { class: "chk" }, `${GLYPH[e.verdict]} ${e.check} ${e.verdict}`), e.text];
    if (e.kind === "attempt") return [h("b", {}, "attempt"), " ", e.text];
    if (e.kind === "halt") return [h("b", {}, "halted "), e.text];
    if (e.kind === "usage") return [h("b", {}, "measured "), e.text];
    if (e.kind === "inferred") return ["inferred: ", e.text];
    if (e.kind === "decision") return [h("b", {}, "decided "), e.text];
    if (e.kind === "refused") return [h("b", {}, "refused "), e.text.replace(/^refused: /, ""), h("div", { class: "notes", style: "font-size:12px" }, "nobody can approve this in a headless run")];
    if (e.kind === "asked") return [h("b", {}, "asked "), e.text.replace(/^asked: /, ""), h("div", { class: "notes", style: "font-size:12px" }, "the agent waits for an answer; the stage clock is stopped")];
    return [e.text];
  }
  function legend() {
    return h("div", { class: "legend" }, h("span", { class: "wit" }, h("i"), "witnessed by conductor"), h("span", { class: "obs" }, h("i"), "observed from the agent"), h("span", { class: "mea" }, h("i"), "measured"), h("span", { class: "inf" }, h("i"), "inferred"));
  }

  function renderSide() {
    const d = state.d, r = d.receipt, s = d.summary;
    side.replaceChildren();
    if (d.live && d.live.waiting) side.append(waitingPanel(d.live.waiting));
    if (!r) {
      side.append(h("div", { class: "panel" }, h("h3", {}, "receipt"), h("p", { class: "notes" }, s.running ? "The receipt is written when the run ends." : "This run has no receipt: it never finished.")));
      if (d.live && !d.live.waiting) side.append(livePanel(d.live));
      return;
    }
    if (r.survivors.length) side.append(h("div", { class: "panel look" }, h("h3", {}, "look here first", h("small", {}, "bugs the tests missed")), ...r.survivors.map((x) => [h("div", { class: "at" }, x.at), h("div", { class: "chg" }, x.change)])));
    side.append(h("div", { class: "panel rows" }, h("h3", {}, "what was proven", h("small", {}, "each of these could have failed")), ...r.checks.map((c) => h("div", { class: "row" }, glyph(c.verdict), h("div", {}, h("div", { class: "claim" }, c.claim), h("div", { class: "detail" }, c.detail))))));
    side.append(h("div", { class: "panel" }, h("h3", {}, "not checked"), h("ul", { class: "notes" }, ...r.not_checked.map((n) => h("li", {}, n)))));
    const i = r.integrity;
    side.append(h("div", { class: "panel" }, h("h3", {}, "how it ran"), h("table", { class: "how" }, ...r.how.map(([k, v]) => h("tr", {}, h("td", {}, k), h("td", {}, v))),
      h("tr", {}, h("td", {}, "record"), h("td", {}, `chain ${i.chain_head.slice(0, 19)}… · ${i.anchored_in ? "anchored in " + i.anchored_in.slice(0, 12) : "not anchored"} · ${i.reproduces ? "re-checks cleanly" : "not re-checked"}`)),
      h("tr", {}, h("td", {}, "verify"), h("td", {}, h("code", {}, `conductor verify ${s.run_id}`))))));
  }
  // The run is paused for a person: a stage's decision, or a tool the agent asked for.
  // Their answer is the one thing this page writes.
  function waitingPanel(w) {
    const field = "width:100%;box-sizing:border-box;font:inherit;background:var(--bg);color:var(--text);border:1px solid var(--line);border-radius:6px;padding:6px";
    const tool = w.ask !== undefined && w.ask !== null;
    const note = h("textarea", { rows: 3, placeholder: tool ? "note for the record: why, or why not (optional)" : "note for the record: what you checked, or why not (required)", style: field + ";margin:8px 0" });
    const by = h("input", { value: w.who, style: field, title: "who decides" });
    const status = h("div", { class: "notes" });
    const send = async (yes) => {
      if (!tool && !note.value.trim()) { status.textContent = "say what you checked, or why not: a decision needs a note"; note.focus(); return; }
      status.textContent = "recording…";
      const url = tool ? `/api/runs/${id}/answer` : `/api/runs/${id}/decide`;
      const body = tool ? { ask: w.ask, allowed: yes, by: by.value, note: note.value } : { stage: w.stage, approved: yes, by: by.value, note: note.value };
      const word = tool ? (yes ? "allowed" : "denied") : (yes ? "approved" : "rejected");
      try {
        const r = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
        status.textContent = r.ok ? `${word}; the run continues` : `not recorded: ${await r.text()}`;
      } catch (e) { status.textContent = "not recorded: " + e.message; }
    };
    return h("div", { class: "panel", style: "border-color:var(--run)" },
      h("h3", {}, `waiting for ${w.who}`, h("small", {}, `stage ${w.stage} · since ${ago(w.since)}`)),
      h("p", { style: "margin:0 0 6px" }, w.question),
      h("div", { class: "notes", style: "font-size:12px" }, tool
        ? "The agent is stopped on this until you answer, and the stage clock is stopped with it. Allow only what you would run yourself."
        : "The evidence is in the ticket and in the lanes to the left. Approve only what you have checked."),
      by, note,
      h("div", { style: "display:flex;gap:8px" }, h("button", { class: "on", onclick: () => send(true) }, tool ? "✓ allow" : "✓ approve"), h("button", { onclick: () => send(false) }, tool ? "✗ deny" : "✗ reject")),
      status);
  }
  function livePanel(l) {
    return h("div", { class: "panel rows" }, h("h3", {}, "checks now", h("small", {}, "run by conductor, not by the agent")), ...l.checks.map((c) => h("div", { class: "row" }, glyph(c.status), h("div", {}, h("div", { class: "claim" }, c.name), h("div", { class: "detail" }, c.detail)))), l.note ? h("p", { class: "notes" }, l.note) : null);
  }

  render(); renderControls();
  if (d.summary.running) {
    const poll = async () => {
      const t = state.d.timeline, since = t.entries.length ? t.entries[t.entries.length - 1].seq : 0;
      let tail;
      try { tail = await get(`/api/runs/${id}/tail?since=${since}`); } catch { return; }
      t.entries.push(...tail.entries); t.attempts = tail.attempts; t.cost_usd = tail.cost_usd; t.tokens = tail.tokens; t.catches = tail.catches;
      state.d.summary = tail.summary; state.d.live = tail.live; state.d.diffs = tail.diffs;
      if (tail.ended) { clearInterval(timer); try { state.d = await get(`/api/runs/${id}`); } catch {} }
      render(); if (state.follow) renderControls();
    };
    const timer = setInterval(poll, 1000);
    stop = () => { clearInterval(timer); clearInterval(state.timer); };
  } else stop = () => clearInterval(state.timer);
}

// ── stats ──────────────────────────────────────────────────────────────────
async function statsPage(main) {
  let st;
  try { st = await get("/api/stats"); } catch (e) { main.append(h("div", { class: "empty" }, e.message)); return; }
  if (!st.runs.length) { main.append(h("div", { class: "empty" }, "No runs yet.")); return; }
  main.append(tiles(st.runs));
  const m = (name) => st.metrics.find((x) => x.name === name)?.points || [];
  // Points carry every attribute the metric has; a chart sums over the ones it doesn't show.
  const sumBy = (points, key) => { const out = new Map(); for (const p of points) { const k = key(p.attrs); out.set(k, (out.get(k) || 0) + p.value); } return [...out].map(([label, value]) => ({ label, value })); };
  const charts = h("div", { class: "charts" });
  // Catches by check: what stops agents. One hue: magnitude.
  const catches = sumBy(m("conductor.catches"), (a) => a.check || "?").sort((a, b) => b.value - a.value);
  charts.append(barChart("what catches the agent", "attempts stopped, by check", catches, { color: "var(--fail)" }));
  // Attempts by stage and outcome: stacked, status colours (state, not identity).
  const byStage = new Map();
  for (const p of m("conductor.attempts")) { const s = p.attrs.stage || "?"; const o = p.attrs.outcome || "?"; byStage.set(s, { ...(byStage.get(s) || {}), [o]: (byStage.get(s)?.[o] || 0) + p.value }); }
  const outcomes = ["passed", "check_failed", "did_not_finish", "halted"];
  charts.append(stackChart("attempts by stage", "how each stage's tries ended", [...byStage].map(([s, o]) => ({ label: s, parts: outcomes.map((k) => o[k] || 0) })), outcomes.map((o) => o.replace("_", " ")), ["var(--pass)", "var(--fail)", "var(--warn)", "var(--blocked)"]));
  // Cost per run, in time order: one hue.
  const chrono = [...st.runs].filter((r) => !r.running).sort((a, b) => a.started_at.localeCompare(b.started_at));
  charts.append(barChart("cost per run", "USD, oldest to newest", chrono.map((r) => ({ label: r.run_id.slice(-5), value: r.cost_usd, title: `${r.work}\n${money(r.cost_usd)} · ${r.verdict}` })), { color: "var(--accent)", fmt: money }));
  // Evidence mix: how much of the record is witnessed vs only observed or inferred.
  const ev = m("conductor.events");
  const srcs = ["witnessed", "observed", "measured", "inferred"];
  const mix = srcs.map((s) => ev.filter((p) => p.attrs.source === s).reduce((a, p) => a + p.value, 0));
  charts.append(stackChart("evidence mix", "recorded events by how conductor knows them", [{ label: "all runs", parts: mix }], srcs, ["var(--accent)", "var(--observed)", "var(--measured)", "var(--inferred)"], true));
  // Checks run by verdict.
  const checks = new Map();
  for (const p of m("conductor.checks")) { const c = p.attrs.check || "?"; const v = p.attrs.verdict || "?"; checks.set(c, { ...(checks.get(c) || {}), [v]: (checks.get(c)?.[v] || 0) + p.value }); }
  const verdicts = ["passed", "failed", "flaky", "unwitnessed"];
  charts.append(stackChart("checks by verdict", "every check conductor ran", [...checks].map(([c, v]) => ({ label: c, parts: verdicts.map((k) => v[k] || 0) })), verdicts, ["var(--pass)", "var(--fail)", "var(--warn)", "var(--blocked)"]));
  // Stage durations.
  const obs = new Map();
  for (const p of m("conductor.stage.duration")) { const k = p.attrs.stage || "?"; obs.set(k, [...(obs.get(k) || []), ...(p.observations || [p.value])]); }
  const durs = [...obs].map(([label, o]) => ({ label, value: o.reduce((a, b) => a + b, 0) / o.length }));
  charts.append(barChart("stage wall clock", "average per stage", durs, { color: "var(--accent)", fmt: (v) => dur(v) }));
  main.append(charts);
}

const tip = $("#tip");
function showTip(ev, html) { tip.innerHTML = html; tip.hidden = false; moveTip(ev); }
function moveTip(ev) { tip.style.left = Math.min(ev.clientX + 12, innerWidth - 330) + "px"; tip.style.top = ev.clientY + 12 + "px"; }
function hideTip() { tip.hidden = true; }

function chartBox(title, sub, svg, table, legendEl) {
  const showTable = h("button", { onclick: () => { const on = table.hidden; table.hidden = !on; svg.hidden = on; showTable.classList.toggle("on", on); } }, "table");
  table.hidden = true;
  return h("div", { class: "panel chart" }, h("h3", {}, title, h("small", {}, sub), h("span", { class: "tools" }, showTable)), svg, table, legendEl || null);
}
function barChart(title, sub, data, { color, fmt = (v) => String(Math.round(v)) }) {
  if (!data.length) return chartBox(title, sub, h("div", { class: "notes" }, "nothing yet"), h("table"));
  const W = 360, rowH = 22, labelW = 110, H = data.length * rowH + 8, max = Math.max(...data.map((d) => d.value), 1e-9);
  const svgNS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(svgNS, "svg"); svg.setAttribute("viewBox", `0 0 ${W} ${H}`); svg.setAttribute("role", "img"); svg.setAttribute("aria-label", title);
  data.forEach((d, i) => {
    const y = i * rowH + 4, w = Math.max(2, ((W - labelW - 50) * d.value) / max);
    const g = document.createElementNS(svgNS, "g");
    const hit = document.createElementNS(svgNS, "rect"); hit.setAttribute("class", "hit"); hit.setAttribute("x", 0); hit.setAttribute("y", y); hit.setAttribute("width", W); hit.setAttribute("height", rowH);
    hit.addEventListener("mouseenter", (ev) => showTip(ev, `<b>${esc(d.label)}</b>${esc(d.title || fmt(d.value))}`.replace(/\n/g, "<br>"))); hit.addEventListener("mousemove", moveTip); hit.addEventListener("mouseleave", hideTip);
    const t = document.createElementNS(svgNS, "text"); t.setAttribute("x", labelW - 8); t.setAttribute("y", y + 15); t.setAttribute("text-anchor", "end"); t.textContent = d.label.length > 16 ? d.label.slice(0, 15) + "…" : d.label;
    const bar = document.createElementNS(svgNS, "rect"); bar.setAttribute("class", "bar"); bar.setAttribute("x", labelW); bar.setAttribute("y", y + 4); bar.setAttribute("width", w); bar.setAttribute("height", rowH - 8); bar.setAttribute("fill", color);
    const v = document.createElementNS(svgNS, "text"); v.setAttribute("class", "val"); v.setAttribute("x", labelW + w + 6); v.setAttribute("y", y + 15); v.textContent = fmt(d.value);
    g.append(hit, t, bar, v); svg.append(g);
  });
  const table = h("table", {}, h("tr", {}, h("th", {}, "item"), h("th", { class: "num" }, "value")), ...data.map((d) => h("tr", {}, h("td", {}, d.label), h("td", { class: "num" }, fmt(d.value)))));
  return chartBox(title, sub, svg, table);
}
function stackChart(title, sub, data, names, colors, horizontalTotals) {
  if (!data.length || !data.some((d) => d.parts.some((p) => p > 0))) return chartBox(title, sub, h("div", { class: "notes" }, "nothing yet"), h("table"));
  const W = 360, rowH = 22, labelW = 110, H = data.length * rowH + 8, max = Math.max(...data.map((d) => d.parts.reduce((a, b) => a + b, 0)), 1e-9);
  const svgNS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(svgNS, "svg"); svg.setAttribute("viewBox", `0 0 ${W} ${H}`); svg.setAttribute("role", "img"); svg.setAttribute("aria-label", title);
  data.forEach((d, i) => {
    const y = i * rowH + 4, total = d.parts.reduce((a, b) => a + b, 0);
    const t = document.createElementNS(svgNS, "text"); t.setAttribute("x", labelW - 8); t.setAttribute("y", y + 15); t.setAttribute("text-anchor", "end"); t.textContent = d.label.length > 16 ? d.label.slice(0, 15) + "…" : d.label; svg.append(t);
    let x = labelW;
    d.parts.forEach((p, j) => {
      if (!p) return;
      const w = ((W - labelW - 50) * p) / max;
      const r = document.createElementNS(svgNS, "rect"); r.setAttribute("class", "bar"); r.setAttribute("x", x); r.setAttribute("y", y + 4); r.setAttribute("width", Math.max(0, w - 2)); r.setAttribute("height", rowH - 8); r.setAttribute("fill", colors[j]);
      r.addEventListener("mouseenter", (ev) => showTip(ev, `<b>${esc(d.label)} · ${esc(names[j])}</b>${Math.round(p)} of ${Math.round(total)}${horizontalTotals ? ` (${Math.round((100 * p) / total)}%)` : ""}`)); r.addEventListener("mousemove", moveTip); r.addEventListener("mouseleave", hideTip);
      svg.append(r); x += w;
    });
    const v = document.createElementNS(svgNS, "text"); v.setAttribute("class", "val"); v.setAttribute("x", x + 6); v.setAttribute("y", y + 15); v.textContent = String(Math.round(total)); svg.append(v);
  });
  const legend = h("div", { class: "legend-row" }, ...names.map((n, j) => h("span", {}, h("i", { style: `background:${colors[j]}` }), n)));
  const table = h("table", {}, h("tr", {}, h("th", {}, "item"), ...names.map((n) => h("th", { class: "num" }, n))), ...data.map((d) => h("tr", {}, h("td", {}, d.label), ...d.parts.map((p) => h("td", { class: "num" }, Math.round(p))))));
  return chartBox(title, sub, svg, table, legend);
}
