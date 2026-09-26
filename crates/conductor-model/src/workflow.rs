//! Workflow files (SPEC §13.1): parsing and the checks that run before anything starts.
//!
//! Validation is where a workflow's mistakes are cheapest. Everything here is pure: it reads
//! the text it is given and reports, and it never guesses what a malformed field meant.
//! Unknown fields are rejected, because a misspelt `frozen:` that silently does nothing is
//! exactly the kind of gap an agent would walk through.

use globset::{Glob, GlobMatcher};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Each stage's outputs: output id to declared path.
type Outputs<'a> = HashMap<&'a str, BTreeMap<&'a str, &'a str>>;

/// herdr rejects agent start timeouts outside this range (SPEC §11.1).
pub const HERDR_TIMEOUT_MS: std::ops::RangeInclusive<u64> = 3001..=300_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub id: String,
    pub version: u32,
    pub kind: Kind,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub herdr: HerdrPolicy,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub defaults: Defaults,
    /// Commands run once in the run's fresh worktree before any stage, for what a checkout
    /// doesn't carry: `[["npm", "ci"]]`. Each must exit 0, or the run halts.
    #[serde(default)]
    pub setup: Vec<Vec<String>>,
    pub stages: Vec<Stage>,
    #[serde(default)]
    pub teardown: Vec<Teardown>,
    /// What happens to a run that passes: `conductor deliver` runs automatically.
    #[serde(default)]
    pub deliver: Option<Deliver>,
}

/// Delivering a passed run: push its branch and open a pull request with its receipt.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deliver {
    /// Open a pull request; `false` only pushes the branch.
    #[serde(default = "yes")]
    pub pr: bool,
    /// Open it as a draft, for a person to mark ready.
    #[serde(default = "yes")]
    pub draft: bool,
    /// The branch to merge into; the remote's default branch when unset.
    pub base: Option<String>,
    #[serde(default = "default_remote")]
    pub remote: String,
}

impl Default for Deliver {
    fn default() -> Self {
        Deliver {
            pr: true,
            draft: true,
            base: None,
            remote: default_remote(),
        }
    }
}

fn default_remote() -> String {
    "origin".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Build,
    Adhoc,
    Qa,
    Troubleshoot,
    Analyse,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Build => "build",
            Kind::Adhoc => "adhoc",
            Kind::Qa => "qa",
            Kind::Troubleshoot => "troubleshoot",
            Kind::Analyse => "analyse",
        }
    }

    /// The release a kind arrives in, or `None` when it is available now.
    pub fn arrives_in(self) -> Option<&'static str> {
        match self {
            Kind::Build => None,
            Kind::Adhoc | Kind::Qa => Some("v0.2"),
            Kind::Troubleshoot | Kind::Analyse => Some("v0.3"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    pub conductor: Option<String>,
    pub herdr_protocol: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    #[serde(default = "default_artifact_root")]
    pub artifact_root: String,
    #[serde(default = "one")]
    pub max_parallel: u32,
    #[serde(default)]
    pub approvals: Approvals,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            artifact_root: default_artifact_root(),
            max_parallel: 1,
            approvals: Approvals::default(),
        }
    }
}

fn default_artifact_root() -> String {
    ".conductor/{{run_id}}".into()
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approvals {
    #[default]
    Manual,
    Auto,
}

/// Who may drive herdr tabs and panes (SPEC §11.2).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HerdrPolicy {
    #[serde(default = "yes")]
    pub tab_per_run: bool,
    #[serde(default = "yes")]
    pub close_passed_panes: bool,
    #[serde(default)]
    pub agent_control: AgentControl,
    #[serde(default)]
    pub agents_may_start_agents: bool,
}

impl Default for HerdrPolicy {
    fn default() -> Self {
        HerdrPolicy {
            tab_per_run: true,
            close_passed_panes: true,
            agent_control: AgentControl::default(),
            agents_may_start_agents: false,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentControl {
    None,
    #[default]
    OwnTab,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub advisory_max_cost_usd: Option<f64>,
    pub max_stage_wall_clock_sec: Option<u64>,
    pub max_mutation_wall_clock_sec: Option<u64>,
    pub max_prompts_per_stage: Option<u32>,
    pub max_total_wall_clock_sec: Option<u64>,
    #[serde(default)]
    pub on_exceeded: OnExceeded,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExceeded {
    #[default]
    HaltBeforeNextStage,
    HaltNow,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub retries: Retries,
    #[serde(default)]
    pub on_failure: Option<String>,
    #[serde(default)]
    pub on_blocked: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retries {
    pub max: u32,
    pub ladder: Vec<Rung>,
}

impl Default for Retries {
    fn default() -> Self {
        Retries {
            max: 2,
            ladder: vec![Rung::InContext, Rung::Fresh, Rung::CrossKind],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    InContext,
    Fresh,
    CrossKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub id: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub agent: Agent,
    #[serde(default)]
    pub context: Context,
    pub prompt_file: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub outputs: Vec<Output>,
    pub gate: Option<Gate>,
    #[serde(default)]
    pub gates: Vec<Gate>,
    pub on_failure: Option<OnFailure>,
    pub timeout_ms: Option<u64>,
    /// Where this stage's evidence is written as it happens: every check's command, verdict
    /// and output, plus the files named, appended to a ticket in the repository.
    pub evidence: Option<Evidence>,
}

/// Evidence captured while a stage runs, appended to a file the reviewer already reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// The file to append to, relative to the repository: a path, or `{{spec}}` for the
    /// file `conductor run --spec` was given (the ticket).
    pub to: String,
    /// Files whose last lines are evidence too (an application log).
    #[serde(default)]
    pub files: Vec<String>,
}

impl Stage {
    /// A stage a person does: the run waits for their decision.
    pub fn is_human(&self) -> bool {
        self.agent.kind == "human"
    }

    /// `gate:` and `gates:` together, in that order.
    pub fn all_gates(&self) -> impl Iterator<Item = &Gate> {
        self.gate.iter().chain(self.gates.iter())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// `claude`, `codex`, or `script` (a command that stands in for an agent, used for
    /// deterministic tests and demos).
    pub kind: String,
    /// The command a `script` agent runs, in the stage's worktree.
    #[serde(default)]
    pub command: Vec<String>,
    pub model: Option<String>,
    /// How much the agent may do without asking. Defaults to accepting file edits only.
    #[serde(default)]
    pub permission_mode: PermissionMode,
    /// Extra tools the agent may use without asking, in the agent's own syntax, such as
    /// `Bash(cargo test:*)`.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// For a `human` stage: who decides, as the receipt should name them ("PO", "QA lead").
    pub who: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    #[default]
    AcceptEdits,
    /// The agent may run anything. Only for disposable environments.
    Bypass,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Context {
    /// A new agent process that never sees earlier stages' conversations.
    #[default]
    Fresh,
    Continue,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub frozen: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub id: String,
    pub path: String,
    #[serde(default = "yes")]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnFailure {
    pub action: String,
    pub max: Option<u32>,
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Teardown {
    pub id: String,
    #[serde(default)]
    pub always: bool,
}

/// A check conductor runs in its own process (SPEC §9.3).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Gate {
    Scope,
    FileNonempty {
        #[serde(rename = "ref")]
        output: String,
    },
    Schema {
        #[serde(rename = "ref")]
        output: String,
        schema: String,
    },
    CommandAssert {
        command: Vec<String>,
        parser: Parser,
        #[serde(default)]
        assert: Vec<String>,
        #[serde(default)]
        reruns: u32,
        /// For `parser: junit_xml`, where the command writes its report, when the command
        /// can't be told (`{{report}}` in the command is the better way): a path or glob
        /// relative to the repository, such as `target/surefire-reports/*.xml`.
        #[serde(default)]
        report: Option<String>,
    },
    Mutation {
        tool: MutationTool,
        #[serde(default = "yes")]
        in_diff: bool,
        min_score: f64,
    },
}

impl Gate {
    pub fn name(&self) -> &'static str {
        match self {
            Gate::Scope => "scope",
            Gate::FileNonempty { .. } => "file_nonempty",
            Gate::Schema { .. } => "schema",
            Gate::CommandAssert { .. } => "command_assert",
            Gate::Mutation { .. } => "mutation",
        }
    }
}

/// In `evidence.to`, the file `conductor run --spec` was given: the ticket.
pub const SPEC: &str = "{{spec}}";

/// In a `junit_xml` check's command, the path of a fresh file conductor reads the report
/// from, outside the repository.
pub const REPORT: &str = "{{report}}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Parser {
    CargoJson,
    JunitXml,
    /// No output is read: the command must exit 0 (a formatter or linter check).
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationTool {
    CargoMutants,
}

/// `.conductor/policy.yaml`: rules the repository sets for every workflow (PRODUCT J5).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Paths no agent may change.
    #[serde(default)]
    pub protected: Vec<String>,
    /// Files build tools rewrite, which any stage may change (reported, never reviewed).
    /// Unset means [`DEFAULT_GENERATED`]; `[]` means none.
    pub generated: Option<Vec<String>>,
    pub min_mutation_score: Option<f64>,
}

/// Lockfiles that running a project's own tests or build can create or rewrite.
pub const DEFAULT_GENERATED: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
    "poetry.lock",
    "uv.lock",
];

impl Policy {
    pub fn generated(&self) -> Vec<String> {
        match &self.generated {
            Some(g) => g.clone(),
            None => DEFAULT_GENERATED.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn parse(text: &str) -> Result<Self, ParseError> {
        Ok(serde_norway::from_str(text)?)
    }
}

/// One problem, with where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub at: String,
    pub message: String,
}

/// What validation found. A workflow runs only when `errors` is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, at: impl Into<String>, message: impl Into<String>) {
        self.errors.push(Issue {
            at: at.into(),
            message: message.into(),
        });
    }

    fn warn(&mut self, at: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(Issue {
            at: at.into(),
            message: message.into(),
        });
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(#[from] serde_norway::Error);

impl Workflow {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        Ok(serde_norway::from_str(text)?)
    }

    /// Everything that can be decided about a workflow without running it.
    /// Whether a check reads the ticket (`{{spec}}` in a command): such a workflow needs
    /// one, and text alone is not enough.
    pub fn needs_spec(&self) -> bool {
        self.stages.iter().flat_map(|s| s.all_gates()).any(|g| {
            matches!(g, Gate::CommandAssert { command, .. } if command.iter().any(|a| a.contains(SPEC)))
        })
    }

    pub fn validate(&self) -> Report {
        let mut r = Report::default();

        if self.id.trim().is_empty() {
            r.error("id", "must not be empty");
        }
        for (i, argv) in self.setup.iter().enumerate() {
            if argv.first().is_none_or(|p| p.trim().is_empty()) {
                r.error(format!("setup[{i}]"), "must name a program to run");
            }
        }
        if self.version != 1 {
            r.error(
                "version",
                format!("only version 1 exists; found {}", self.version),
            );
        }
        if let Some(release) = self.kind.arrives_in() {
            r.error(
                "kind",
                format!(
                    "`{}` workflows arrive in {release}; this build runs `build` only",
                    self.kind.name()
                ),
            );
        }
        if self.runtime.max_parallel == 0 {
            r.error("runtime.max_parallel", "must be at least 1");
        }
        if self.stages.is_empty() {
            r.error("stages", "a workflow needs at least one stage");
        }
        if let Some(t) = self.defaults.timeout_ms {
            check_timeout(&mut r, "defaults.timeout_ms", t);
        }
        if self.defaults.retries.ladder.is_empty() && self.defaults.retries.max > 0 {
            r.error(
                "defaults.retries.ladder",
                "retries are allowed but the ladder names no rungs",
            );
        }
        for (field, value) in [
            (
                "budget.max_stage_wall_clock_sec",
                self.budget.max_stage_wall_clock_sec,
            ),
            (
                "budget.max_mutation_wall_clock_sec",
                self.budget.max_mutation_wall_clock_sec,
            ),
            (
                "budget.max_total_wall_clock_sec",
                self.budget.max_total_wall_clock_sec,
            ),
        ] {
            if value == Some(0) {
                r.error(field, "a zero budget would halt every run before it starts");
            }
        }
        if let Some(cost) = self.budget.advisory_max_cost_usd
            && !(cost.is_finite() && cost > 0.0)
        {
            r.error("budget.advisory_max_cost_usd", "must be a positive amount");
        }

        let mut outputs: Outputs = HashMap::new();
        let mut seen = BTreeSet::new();
        for (i, s) in self.stages.iter().enumerate() {
            let at = format!("stages[{i}]");
            if !seen.insert(s.id.as_str()) {
                r.error(
                    format!("{at}.id"),
                    format!("stage `{}` is defined twice", s.id),
                );
            }
            outputs
                .entry(s.id.as_str())
                .or_default()
                .extend(s.outputs.iter().map(|o| (o.id.as_str(), o.path.as_str())));
        }

        for (i, s) in self.stages.iter().enumerate() {
            let at = format!("stages[{i}]");
            for d in &s.depends_on {
                if d == &s.id {
                    r.error(
                        format!("{at}.depends_on"),
                        format!("stage `{}` depends on itself", s.id),
                    );
                } else if !seen.contains(d.as_str()) {
                    r.error(
                        format!("{at}.depends_on"),
                        format!("no stage is called `{d}`"),
                    );
                }
            }
            if let Some(t) = s.timeout_ms {
                check_timeout(&mut r, format!("{at}.timeout_ms"), t);
            }
            self.validate_stage(&mut r, &at, s, &outputs);
        }

        if let Some(cycle) = find_cycle(&self.stages) {
            r.error(
                "stages",
                format!(
                    "stages depend on each other in a loop: {}",
                    cycle.join(" → ")
                ),
            );
        }

        if self.kind == Kind::Build {
            self.separation_of_duties(&mut r);
        }
        r
    }

    fn validate_stage(&self, r: &mut Report, at: &str, s: &Stage, outputs: &Outputs) {
        // A check the agent can't run itself leaves it guessing what the check wants. One
        // run spent 15 minutes hand-formatting files because `cargo fmt` wasn't allowed.
        if !s.agent.allowed_tools.is_empty() {
            for g in s.all_gates() {
                if let Gate::CommandAssert { command, .. } = g
                    && !agent_may_run(&s.agent.allowed_tools, command)
                {
                    r.warn(
                        format!("{at}.agent.allowed_tools"),
                        format!(
                            "`{}` is checked, but the agent may not run it; add `Bash({}:*)` so it can check its own work",
                            command.join(" "),
                            command.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
                        ),
                    );
                }
            }
        }
        if s.agent.kind == "script" && s.agent.command.is_empty() {
            r.error(
                format!("{at}.agent.command"),
                "a `script` agent needs a command to run",
            );
        }
        if s.is_human() {
            if s.all_gates().next().is_some() {
                r.error(
                    format!("{at}.gates"),
                    "a `human` stage is the check: the person's decision passes or fails it, so it takes no gates",
                );
            }
            if s.agent.who.as_deref().is_none_or(|w| w.trim().is_empty()) {
                r.error(
                    format!("{at}.agent.who"),
                    "say who decides, as the receipt should name them (`who: PO`)",
                );
            }
            if s.prompt_file.is_none() {
                r.warn(
                    format!("{at}.prompt_file"),
                    "a `human` stage's prompt is what the person is asked to decide; without one they see only the evidence",
                );
            }
        }
        if let Some(e) = &s.evidence {
            let to = e.to.trim();
            if to != SPEC
                && (to.starts_with('/') || to.split('/').any(|c| c == "..") || to.contains("{{"))
            {
                r.error(
                    format!("{at}.evidence.to"),
                    format!("must be a path inside the repository, or `{SPEC}` for the ticket the run was given"),
                );
            }
            for (j, f) in e.files.iter().enumerate() {
                if f.starts_with('/') || f.split('/').any(|c| c == "..") {
                    r.error(
                        format!("{at}.evidence.files[{j}]"),
                        "must be a path inside the repository",
                    );
                }
            }
        }
        if s.agent.kind.trim().is_empty() {
            r.error(
                format!("{at}.agent.kind"),
                "must name an agent, such as `claude` or `codex`",
            );
        }
        let mut out_ids = BTreeSet::new();
        for (j, o) in s.outputs.iter().enumerate() {
            if !out_ids.insert(o.id.as_str()) {
                r.error(
                    format!("{at}.outputs[{j}].id"),
                    format!("output `{}` is declared twice", o.id),
                );
            }
            check_refs(r, &format!("{at}.outputs[{j}].path"), &o.path, outputs);
        }
        for (k, v) in &s.inputs {
            check_refs(r, &format!("{at}.inputs.{k}"), v, outputs);
        }

        let write = self.compile_all(r, &format!("{at}.scope.write"), &s.scope.write, outputs);
        for (j, raw) in s.scope.frozen.iter().enumerate() {
            check_refs(r, &format!("{at}.scope.frozen[{j}]"), raw, outputs);
            let f = &self.resolve(raw, outputs);
            if Glob::new(f).is_err() {
                r.error(
                    format!("{at}.scope.frozen[{j}]"),
                    format!("`{f}` is not a valid path pattern"),
                );
                continue;
            }
            // A frozen literal the same stage may write is a contradiction the scope gate
            // would resolve in whichever direction it happened to check first.
            if !f.contains('*')
                && let Some(w) = write.iter().find(|(_, m)| m.is_match(f))
            {
                r.error(
                    format!("{at}.scope.frozen[{j}]"),
                    format!(
                        "`{f}` is frozen, but this stage may also write it through `{}`",
                        w.0
                    ),
                );
            }
        }

        let gates: Vec<&Gate> = s.all_gates().collect();
        if gates.is_empty() && !s.is_human() {
            r.warn(
                at.to_string(),
                format!(
                    "stage `{}` has no checks, so its receipt can prove nothing about it",
                    s.id
                ),
            );
        }
        if !s.scope.write.is_empty()
            && !gates.iter().any(|g| matches!(g, Gate::Scope))
            && !s.scope.frozen.is_empty()
        {
            r.warn(
                format!("{at}.gates"),
                "frozen paths are declared but no `scope` gate checks them",
            );
        }
        for (j, g) in gates.iter().enumerate() {
            let gat = format!("{at}.gates[{j}]");
            match g {
                Gate::FileNonempty { output } | Gate::Schema { output, .. } => {
                    if !out_ids.contains(output.as_str()) {
                        r.error(
                            format!("{gat}.ref"),
                            format!("stage `{}` has no output called `{output}`", s.id),
                        );
                    }
                }
                Gate::CommandAssert {
                    command,
                    reruns,
                    parser,
                    assert,
                    report,
                } => {
                    if command.is_empty() || command[0].trim().is_empty() {
                        r.error(format!("{gat}.command"), "must name a program to run");
                    }
                    let templated = command.iter().any(|a| a.contains(REPORT));
                    match (parser, templated, report) {
                        (Parser::JunitXml, false, None) => r.error(
                            format!("{gat}.command"),
                            format!(
                                "`parser: junit_xml` needs to know where the report goes: put `{REPORT}` \
                                 in the command where the file path belongs, or name it with `report:`"
                            ),
                        ),
                        (Parser::JunitXml, true, Some(_)) => r.error(
                            format!("{gat}.report"),
                            format!("the command already writes to `{REPORT}`; drop `report:`"),
                        ),
                        (Parser::JunitXml, _, _) => {}
                        (_, true, _) => r.error(
                            format!("{gat}.command"),
                            format!("`{REPORT}` only means something with `parser: junit_xml`"),
                        ),
                        (_, _, Some(_)) => r.error(
                            format!("{gat}.report"),
                            "`report:` only applies to `parser: junit_xml`",
                        ),
                        _ => {}
                    }
                    if let Some(p) = report {
                        if p.starts_with('/') || p.split('/').any(|c| c == "..") {
                            r.error(
                                format!("{gat}.report"),
                                "must be a path inside the repository",
                            );
                        } else if Glob::new(p).is_err() {
                            r.error(
                                format!("{gat}.report"),
                                format!("`{p}` is not a valid path pattern"),
                            );
                        }
                    }
                    if *parser == Parser::Exit && !assert.is_empty() {
                        r.error(
                            format!("{gat}.assert"),
                            "`parser: exit` passes when the command exits 0 and takes no assertions",
                        );
                    }
                    if *reruns > 10 {
                        r.warn(
                            format!("{gat}.reruns"),
                            format!("{reruns} reruns will make every run slow"),
                        );
                    }
                }
                Gate::Mutation {
                    min_score, in_diff, ..
                } => {
                    if !(0.0..=1.0).contains(min_score) {
                        r.error(format!("{gat}.min_score"), "must be between 0 and 1");
                    }
                    if !in_diff {
                        r.warn(format!("{gat}.in_diff"), "mutating the whole crate can take hours; `in_diff: true` is recommended");
                    }
                }
                Gate::Scope => {
                    if s.scope.write.is_empty() {
                        r.error(
                            gat.to_string(),
                            "a `scope` gate needs `scope.write` to say what the stage may change",
                        );
                    }
                }
            }
        }
    }

    fn compile_all(
        &self,
        r: &mut Report,
        at: &str,
        globs: &[String],
        outputs: &Outputs,
    ) -> Vec<(String, GlobMatcher)> {
        let mut out = Vec::new();
        for (j, raw) in globs.iter().enumerate() {
            check_refs(r, &format!("{at}[{j}]"), raw, outputs);
            let g = self.resolve(raw, outputs);
            match Glob::new(&g) {
                Ok(glob) => out.push((raw.clone(), glob.compile_matcher())),
                Err(_) => r.error(
                    format!("{at}[{j}]"),
                    format!("`{raw}` is not a valid path pattern"),
                ),
            }
        }
        out
    }

    /// Replaces template references with what they stand for, so a pattern that names another
    /// stage's output can be compared with real paths. `{{run_id}}` becomes a placeholder
    /// segment that matches nothing a stage would write by accident.
    fn resolve(&self, text: &str, outputs: &Outputs) -> String {
        self.resolve_for(text, outputs, "RUN")
    }

    /// `text` with every template replaced for a real run: output references become the
    /// declared paths, `{{run_id}}` the run's id, `{{artifact_root}}` the resolved root.
    pub fn render(&self, text: &str, run_id: &str) -> String {
        let outputs: Outputs = self
            .stages
            .iter()
            .map(|s| {
                (
                    s.id.as_str(),
                    s.outputs
                        .iter()
                        .map(|o| (o.id.as_str(), o.path.as_str()))
                        .collect(),
                )
            })
            .collect();
        self.resolve_for(text, &outputs, run_id)
    }

    fn resolve_for(&self, text: &str, outputs: &Outputs, run_id: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                out.push_str(&rest[start..]);
                return out;
            };
            let key = after[..end].trim();
            let value = match key {
                "run_id" => run_id.to_owned(),
                "artifact_root" => self.resolve_for(&self.runtime.artifact_root, outputs, run_id),
                _ => referenced_outputs(&format!("{{{{{key}}}}}"))
                    .first()
                    .and_then(|(st, o)| outputs.get(st.as_str()).and_then(|m| m.get(o.as_str())))
                    .map(|p| self.resolve_for(p, outputs, run_id))
                    .unwrap_or_default(),
            };
            out.push_str(&value);
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        out
    }

    /// In a build workflow, the stage that writes the tests and the stage that must not touch
    /// them should be different agents. Same agent is allowed, but called out.
    fn separation_of_duties(&self, r: &mut Report) {
        for (i, s) in self.stages.iter().enumerate() {
            // An implementer working against locked tests can still add tests of its own in
            // src/ (a `#[cfg(test)]` module), which no path pattern can see. Counting tests
            // can: only the locked ones may exist after the stage.
            let counts_new_tests = s.all_gates().any(|g| {
                matches!(g, Gate::CommandAssert { assert, .. }
                    if assert.iter().any(|a| a.split_whitespace().collect::<String>() == "tests_new==0"))
            });
            if !s.scope.frozen.is_empty() && !counts_new_tests {
                r.warn(
                    format!("stages[{i}].gates"),
                    format!(
                        "`{}` works against locked tests but could add its own inside src/, where \
                         the scope check can't see them; assert `tests_new == 0`",
                        s.id
                    ),
                );
            }
            for f in &s.scope.frozen {
                for (dep_stage, _) in referenced_outputs(f) {
                    if let Some(author) = self.stages.iter().find(|x| x.id == dep_stage)
                        && author.agent.kind == s.agent.kind
                    {
                        r.warn(
                            format!("stages[{i}].agent.kind"),
                            format!(
                                "`{}` and `{}` are both `{}`; a different agent for the tests makes them stronger evidence",
                                author.id, s.id, s.agent.kind
                            ),
                        );
                    }
                }
            }
        }
    }
}

fn check_timeout(r: &mut Report, at: impl Into<String>, t: u64) {
    if !HERDR_TIMEOUT_MS.contains(&t) {
        r.error(
            at,
            format!(
                "{t} ms is outside herdr's accepted range of more than 3000 and at most 300000"
            ),
        );
    }
}

/// Whether Claude Code's `allowed_tools` let an agent run `command`: a `Bash` entry whose
/// pattern (`Bash(cargo test:*)`, `Bash(cargo fmt)`, `Bash`) covers the command line.
fn agent_may_run(allowed: &[String], command: &[String]) -> bool {
    let line = command.join(" ");
    allowed.iter().any(|t| {
        let t = t.trim();
        if t == "Bash" || t == "Bash(*)" {
            return true;
        }
        let Some(pattern) = t.strip_prefix("Bash(").and_then(|p| p.strip_suffix(')')) else {
            return false;
        };
        match pattern
            .strip_suffix(":*")
            .or_else(|| pattern.strip_suffix('*'))
        {
            Some(prefix) => line.starts_with(prefix.trim_end()),
            None => line == pattern,
        }
    })
}

/// `{{stages.<stage>.outputs.<output>.path}}` references in a string.
fn referenced_outputs(text: &str) -> Vec<(String, String)> {
    templates(text)
        .filter_map(|t| {
            let parts: Vec<&str> = t.split('.').collect();
            match parts.as_slice() {
                ["stages", stage, "outputs", output, "path"] => {
                    Some(((*stage).to_owned(), (*output).to_owned()))
                }
                _ => None,
            }
        })
        .collect()
}

fn templates(text: &str) -> impl Iterator<Item = &str> {
    text.split("{{")
        .skip(1)
        .filter_map(|rest| rest.split_once("}}").map(|(inner, _)| inner.trim()))
}

fn check_refs(r: &mut Report, at: &str, text: &str, outputs: &Outputs) {
    for t in templates(text) {
        if t == "run_id" || t == "artifact_root" {
            continue;
        }
        match referenced_outputs(&format!("{{{{{t}}}}}")).first() {
            Some((stage, output)) => match outputs.get(stage.as_str()) {
                None => r.error(
                    at,
                    format!("refers to stage `{stage}`, which doesn't exist"),
                ),
                Some(outs) if !outs.contains_key(output.as_str()) => r.error(
                    at,
                    format!("stage `{stage}` has no output called `{output}`"),
                ),
                Some(_) => {}
            },
            None => r.error(at, format!("`{{{{{t}}}}}` is not a value conductor knows")),
        }
    }
}

fn find_cycle(stages: &[Stage]) -> Option<Vec<String>> {
    let deps: HashMap<&str, &[String]> = stages
        .iter()
        .map(|s| (s.id.as_str(), s.depends_on.as_slice()))
        .collect();
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit<'a>(
        id: &'a str,
        deps: &HashMap<&'a str, &'a [String]>,
        marks: &mut HashMap<&'a str, Mark>,
        path: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        match marks.get(id) {
            Some(Mark::Done) => return None,
            Some(Mark::Visiting) => {
                let start = path.iter().position(|p| *p == id).unwrap_or(0);
                let mut cycle: Vec<String> =
                    path[start..].iter().map(|s| (*s).to_owned()).collect();
                cycle.push(id.to_owned());
                return Some(cycle);
            }
            None => {}
        }
        marks.insert(id, Mark::Visiting);
        path.push(id);
        for d in deps.get(id).copied().unwrap_or_default() {
            if d != id
                && deps.contains_key(d.as_str())
                && let Some(c) = visit(d.as_str(), deps, marks, path)
            {
                return Some(c);
            }
        }
        path.pop();
        marks.insert(id, Mark::Done);
        None
    }
    let mut marks = HashMap::new();
    for s in stages {
        if let Some(c) = visit(s.id.as_str(), &deps, &mut marks, &mut Vec::new()) {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../examples/build.yaml");

    #[test]
    fn a_junit_check_must_say_where_its_report_goes() {
        let errors = |gate: &str, setup: &str| {
            let wf = format!(
                "id: w\nversion: 1\nkind: build\nsetup: {setup}\nstages:\n  - id: s\n    agent: {{ kind: script, command: [\"true\"] }}\n    gates:\n      - {gate}\n"
            );
            Workflow::parse(&wf)
                .unwrap()
                .validate()
                .errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
        };
        let ok = |gate: &str| assert_eq!(errors(gate, "[]"), Vec::<String>::new(), "{gate}");
        ok(
            r#"{ type: command_assert, command: ["pytest", "--junitxml={{report}}"], parser: junit_xml }"#,
        );
        ok(
            r#"{ type: command_assert, command: ["mvn", "test"], parser: junit_xml, report: "**/surefire-reports/*.xml" }"#,
        );
        let bad = |gate: &str, says: &str| {
            let e = errors(gate, "[]");
            assert!(e.iter().any(|m| m.contains(says)), "{gate}: {e:?}");
        };
        bad(
            r#"{ type: command_assert, command: ["pytest"], parser: junit_xml }"#,
            "needs to know where the report goes",
        );
        bad(
            r#"{ type: command_assert, command: ["pytest", "--junitxml={{report}}"], parser: junit_xml, report: "r.xml" }"#,
            "drop `report:`",
        );
        bad(
            r#"{ type: command_assert, command: ["cargo", "test", "{{report}}"], parser: cargo_json }"#,
            "only means something with `parser: junit_xml`",
        );
        bad(
            r#"{ type: command_assert, command: ["make"], parser: exit, report: "r.xml" }"#,
            "only applies to `parser: junit_xml`",
        );
        bad(
            r#"{ type: command_assert, command: ["mvn"], parser: junit_xml, report: "../elsewhere/*.xml" }"#,
            "inside the repository",
        );
        let e = errors("{ type: scope }", "[[]]");
        assert!(e.iter().any(|m| m == "must name a program to run"), "{e:?}");
    }

    #[test]
    fn a_check_the_agent_may_not_run_is_flagged() {
        let allowed = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cmd = |c: &str| c.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let a = allowed(&["Bash(cargo test:*)", "Read"]);
        assert!(agent_may_run(&a, &cmd("cargo test --workspace")));
        assert!(!agent_may_run(&a, &cmd("cargo fmt --all --check")));
        assert!(agent_may_run(
            &allowed(&["Bash"]),
            &cmd("cargo fmt --all --check")
        ));
        assert!(agent_may_run(
            &allowed(&["Bash(cargo fmt --all --check)"]),
            &cmd("cargo fmt --all --check")
        ));

        let wf = format!(
            "{}\n      - {{ type: command_assert, command: [\"cargo\", \"fmt\", \"--all\", \"--check\"], parser: exit }}\n",
            r#"id: w
version: 1
kind: build
stages:
  - id: s
    agent: { kind: claude, allowed_tools: ["Bash(cargo test:*)"] }
    scope: { write: ["src/**"] }
    gates:
      - type: command_assert
        command: ["cargo", "test"]
        parser: cargo_json
        assert: ["tests_failed == 0"]"#
        );
        let v = Workflow::parse(&wf).unwrap().validate();
        let msgs: Vec<&str> = v.warnings.iter().map(|w| w.message.as_str()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("`cargo fmt --all --check` is checked, but the agent may not run it; add `Bash(cargo fmt:*)`")),
            "{msgs:?}"
        );
        assert!(
            !msgs.iter().any(|m| m.contains("`cargo test` is checked")),
            "{msgs:?}"
        );
    }

    #[test]
    fn an_implementer_that_could_write_its_own_tests_is_flagged() {
        let ok = Workflow::parse(EXAMPLE).unwrap().validate();
        assert!(
            !ok.warnings
                .iter()
                .any(|w| w.message.contains("tests_new == 0")),
            "{:?}",
            ok.warnings
        );
        let loose = EXAMPLE.replace(
            r#"["tests_failed == 0", "tests_new == 0"]"#,
            r#"["tests_failed == 0"]"#,
        );
        assert_ne!(loose, EXAMPLE);
        let v = Workflow::parse(&loose).unwrap().validate();
        assert!(v.is_ok(), "a warning, not an error");
        assert!(
            v.warnings
                .iter()
                .any(|w| w.message.contains("`implement` works against locked tests")),
            "{:?}",
            v.warnings
        );
    }

    fn with(edit: impl FnOnce(&mut String)) -> Report {
        let mut text = EXAMPLE.to_owned();
        edit(&mut text);
        Workflow::parse(&text).expect("still parses").validate()
    }

    fn messages(r: &Report) -> Vec<String> {
        r.errors
            .iter()
            .map(|i| format!("{}: {}", i.at, i.message))
            .collect()
    }

    #[test]
    fn the_shipped_example_is_valid() {
        let r = Workflow::parse(EXAMPLE).unwrap().validate();
        assert!(r.is_ok(), "{:#?}", r.errors);
    }

    #[test]
    fn a_misspelt_field_is_rejected_not_ignored() {
        let text = EXAMPLE.replace("frozen:", "frozn:");
        let err = Workflow::parse(&text).unwrap_err().to_string();
        assert!(err.contains("frozn"), "{err}");
    }

    #[test]
    fn later_kinds_say_when_they_arrive() {
        let r = with(|t| *t = t.replacen("kind: build", "kind: qa", 1));
        assert!(
            messages(&r).iter().any(|m| m.contains("arrive in v0.2")),
            "{:?}",
            messages(&r)
        );
    }

    #[test]
    fn an_unknown_dependency_is_named() {
        let r = with(|t| *t = t.replacen("depends_on: [tests]", "depends_on: [testz]", 1));
        assert!(
            messages(&r)
                .iter()
                .any(|m| m.contains("no stage is called `testz`"))
        );
    }

    #[test]
    fn a_dependency_loop_is_found() {
        let r = with(|t| {
            *t = t.replacen(
                "  - id: spec\n",
                "  - id: spec\n    depends_on: [implement]\n",
                1,
            )
        });
        assert!(
            messages(&r).iter().any(|m| m.contains("loop")),
            "{:?}",
            messages(&r)
        );
    }

    #[test]
    fn a_reference_to_a_missing_output_is_named() {
        let r = with(|t| *t = t.replacen("outputs.tests.path", "outputs.testz.path", 1));
        assert!(
            messages(&r)
                .iter()
                .any(|m| m.contains("no output called `testz`")),
            "{:?}",
            messages(&r)
        );
    }

    #[test]
    fn a_timeout_herdr_would_reject_is_caught_before_the_run() {
        let r = with(|t| *t = t.replacen("timeout_ms: 300000", "timeout_ms: 3000", 1));
        assert!(
            messages(&r)
                .iter()
                .any(|m| m.contains("outside herdr's accepted range"))
        );
    }

    #[test]
    fn freezing_a_path_the_stage_may_write_is_a_contradiction() {
        let r = with(|t| {
            *t = t.replacen(
                "write: [\"src/**\"]",
                "write: [\"src/**\", \"tests/**\"]",
                1,
            )
        });
        assert!(
            messages(&r)
                .iter()
                .any(|m| m.contains("is frozen, but this stage may also write it")),
            "{:?}",
            messages(&r)
        );
    }

    #[test]
    fn a_mutation_score_above_one_is_rejected() {
        let r = with(|t| *t = t.replacen("min_score: 0.7", "min_score: 70", 1));
        assert!(messages(&r).iter().any(|m| m.contains("between 0 and 1")));
    }

    #[test]
    fn the_same_agent_writing_and_implementing_is_called_out() {
        let r = with(|t| *t = t.replacen("kind: codex", "kind: claude", 1));
        assert!(r.is_ok());
        assert!(
            r.warnings
                .iter()
                .any(|w| w.message.contains("both `claude`")),
            "{:?}",
            r.warnings
        );
    }

    #[test]
    fn templates_render_to_real_paths_for_a_run() {
        let wf = Workflow::parse(EXAMPLE).unwrap();
        assert_eq!(
            wf.render("{{stages.tests.outputs.tests.path}}", "01ABC"),
            "tests/generated_tests.rs"
        );
        assert_eq!(
            wf.render("{{stages.spec.outputs.spec.path}}", "01ABC"),
            ".conductor/01ABC/spec.md"
        );
    }

    #[test]
    fn a_script_agent_needs_a_command() {
        let text =
            "id: x\nversion: 1\nkind: build\nstages:\n  - id: a\n    agent: { kind: script }\n";
        let r = Workflow::parse(text).unwrap().validate();
        assert!(messages(&r).iter().any(|m| m.contains("needs a command")));
    }

    #[test]
    fn a_scope_gate_without_a_write_scope_is_an_error() {
        let text = "id: x\nversion: 1\nkind: build\nstages:\n  - id: a\n    agent: { kind: claude }\n    gates: [{ type: scope }]\n";
        let r = Workflow::parse(text).unwrap().validate();
        assert!(
            messages(&r)
                .iter()
                .any(|m| m.contains("needs `scope.write`"))
        );
    }
}
