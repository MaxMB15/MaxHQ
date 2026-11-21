// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading a descriptor: its version, overlays merged over it, and the
//! checks serde can't make.

use serde_norway::Value as Yaml;
use thiserror::Error;

use crate::descriptor::{
    Descriptor, Expr, Predicate, Provisional, Source, AGENTS_ID, FORMAT, MAJOR, MINOR,
    OVERLAY_FORMAT,
};

/// The most a descriptor, run file or other file the engine reads may be.
pub const MAX_FILE: u64 = 4 * 1024 * 1024;

/// How deep a descriptor's predicates may nest.
const MAX_DEPTH: usize = 16;

/// The most a descriptor or overlay may be. Real ones are under 8 KiB.
const MAX_DESCRIPTOR: usize = 64 * 1024;

/// The most nodes a file the engine parses may expand to, aliases counted
/// each time they repeat. A real descriptor has under 2,000.
const MAX_NODES: u64 = 1_000_000;

/// Why a descriptor didn't load.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LoadError {
    /// It isn't YAML or JSON, or doesn't fit the format.
    #[error("{0}")]
    Parse(String),
    /// Its `format` is missing or isn't one MaxHQ reads.
    #[error("{0}")]
    Format(String),
    /// It needs a newer MaxHQ.
    #[error("This workflow needs a newer MaxHQ.")]
    TooNew {
        /// The workflow's id, when the file names one.
        id: Option<String>,
        /// The major version it was written for.
        major: u32,
    },
    /// It parses but breaks a rule of the standard.
    #[error("{0}")]
    Invalid(String),
}

/// A file that parsed and named a format MaxHQ reads.
#[derive(Debug)]
pub(crate) enum Raw {
    /// A descriptor, not yet merged or checked.
    Workflow { id: String, minor: u32, value: Yaml },
    /// An overlay, for the workflow it names.
    Overlay { workflow: String, value: Yaml },
}

/// Reads a descriptor with no overlays.
pub fn load(bytes: &[u8]) -> Result<Descriptor, LoadError> {
    match read(bytes)? {
        Raw::Workflow { minor, value, .. } => finish(value, minor),
        Raw::Overlay { .. } => Err(LoadError::Format(
            "This is an overlay, not a workflow.".into(),
        )),
    }
}

/// Whether YAML from a repository is safe to parse. See
/// [`maxhq_protocol::yaml`].
pub(crate) fn check_yaml(bytes: &[u8]) -> Result<(), &'static str> {
    let document = serde_norway::Deserializer::from_slice(bytes);
    maxhq_protocol::yaml::check(bytes, document, MAX_NODES)
}

/// Parses a file and sorts it by its `format`.
pub(crate) fn read(bytes: &[u8]) -> Result<Raw, LoadError> {
    if bytes.len() > MAX_DESCRIPTOR {
        return Err(LoadError::Parse("The file is too large.".into()));
    }
    check_yaml(bytes).map_err(|why| LoadError::Parse(why.to_owned()))?;
    let value: Yaml =
        serde_norway::from_slice(bytes).map_err(|e| LoadError::Parse(e.to_string()))?;
    let text = |key: &str| value.get(key).and_then(Yaml::as_str).map(str::to_owned);
    let format = text("format").ok_or_else(|| LoadError::Format("It has no `format`.".into()))?;
    let (name, version) = format
        .split_once('/')
        .ok_or_else(|| LoadError::Format(format!("`{format}` isn't a format.")))?;
    let (major, minor) = version
        .split_once('.')
        .and_then(|(major, minor)| Some((major.parse().ok()?, minor.parse().ok()?)))
        .ok_or_else(|| LoadError::Format(format!("`{format}` has no version.")))?;
    if name != FORMAT && name != OVERLAY_FORMAT {
        return Err(LoadError::Format(format!("`{name}` isn't a MaxHQ format.")));
    }
    let id = text(if name == FORMAT { "id" } else { "workflow" });
    if major > MAJOR {
        return Err(LoadError::TooNew { id, major });
    }
    // Each major from 2 on brings its converter from the one before here.
    if major < MAJOR {
        return Err(LoadError::Format(format!(
            "`{format}` isn't a version MaxHQ reads."
        )));
    }
    let id = id.ok_or_else(|| LoadError::Invalid("It has no `id`.".into()))?;
    Ok(if name == FORMAT {
        Raw::Workflow { id, minor, value }
    } else {
        Raw::Overlay {
            workflow: id,
            value,
        }
    })
}

/// Merges an overlay over a descriptor, as a JSON merge patch (RFC 7396)
/// that keeps the descriptor's key order.
pub(crate) fn merge(base: &mut Yaml, patch: Yaml) {
    let Yaml::Mapping(patch) = patch else {
        *base = patch;
        return;
    };
    if !base.is_mapping() {
        *base = Yaml::Mapping(Default::default());
    }
    let Yaml::Mapping(base) = base else {
        unreachable!()
    };
    for (key, value) in patch {
        if matches!(key.as_str(), Some("format" | "workflow")) {
            continue;
        }
        if value.is_null() {
            base.remove(&key);
        } else if let Some(slot) = base.get_mut(&key) {
            merge(slot, value);
        } else {
            base.insert(key, value);
        }
    }
}

/// Turns a merged descriptor into its types and checks it. A key the
/// standard lacks is refused, unless it's an `x-` extension or the
/// descriptor is for a newer minor, which may have added it.
pub(crate) fn finish(value: Yaml, minor: u32) -> Result<Descriptor, LoadError> {
    let mut unknown = None;
    let mut descriptor: Descriptor = serde_ignored::deserialize(value, |path| {
        let path = key_path(&path);
        if unknown.is_none() && !(path.len() == 1 && path[0].starts_with("x-")) {
            unknown = Some(path.join("."));
        }
    })
    .map_err(|e| LoadError::Parse(e.to_string()))?;
    descriptor.minor = minor;
    if let Some(path) = unknown.filter(|_| !descriptor.newer()) {
        return Err(invalid(format!(
            "`{path}` isn't a key of {FORMAT}/{MAJOR}.{MINOR}."
        )));
    }
    check(&descriptor)?;
    Ok(descriptor)
}

/// The keys and list positions down to an ignored key.
fn key_path(path: &serde_ignored::Path) -> Vec<String> {
    use serde_ignored::Path;
    match path {
        Path::Root => Vec::new(),
        Path::Seq { parent, index } => {
            let mut out = key_path(parent);
            out.push(index.to_string());
            out
        }
        Path::Map { parent, key } => {
            let mut out = key_path(parent);
            out.push(key.clone());
            out
        }
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => key_path(parent),
    }
}

fn invalid(text: impl Into<String>) -> LoadError {
    LoadError::Invalid(text.into())
}

/// The rules of the standard that the types alone don't hold.
fn check(d: &Descriptor) -> Result<(), LoadError> {
    let id_ok =
        d.id.chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && d.id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
    if !id_ok {
        return Err(invalid(format!("`{}` isn't a workflow id.", d.id)));
    }
    if d.id == AGENTS_ID || d.id.starts_with("maxhq.") {
        return Err(invalid("Ids that start with `maxhq.` are MaxHQ's own."));
    }
    let glob = d
        .runs
        .glob
        .strip_prefix("{project}/")
        .unwrap_or(&d.runs.glob);
    if glob.is_empty() || glob.starts_with('/') || glob.split('/').any(|part| part == "..") {
        return Err(invalid("`runs.glob` must stay inside the project."));
    }
    let prefix = d.env_prefix();
    for (name, _) in d.runs.env.iter() {
        let chars_ok = name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !name.starts_with(&prefix) || name.len() == prefix.len() || !chars_ok {
            return Err(invalid(format!(
                "`{name}` can't be set: names in `runs.env` start with `{prefix}`."
            )));
        }
    }
    for link in &d.links {
        if !local_url(&link.url) {
            return Err(invalid(format!(
                "`{}` isn't a link MaxHQ opens: only http://127.0.0.1 and http://localhost.",
                link.url
            )));
        }
    }
    for expr in exprs(d) {
        check_expr(expr, d.newer())?;
    }
    if predicates(d).iter().any(|p| p.depth() > MAX_DEPTH) {
        return Err(invalid(format!(
            "A `when` nests more than {MAX_DEPTH} levels deep."
        )));
    }
    Ok(())
}

/// Every predicate in a descriptor.
fn predicates(d: &Descriptor) -> Vec<&Predicate> {
    let labels = d.badges.iter().map(|b| &b.label).chain(&d.header);
    d.rules
        .iter()
        .map(|r| &r.when)
        .chain(d.rules.iter().filter_map(|r| match &r.provisional {
            Some(Provisional::When(p)) => Some(p.as_ref()),
            _ => None,
        }))
        .chain(labels.filter_map(|l| l.when.as_ref()))
        .chain(d.links.iter().filter_map(|l| l.when.as_ref()))
        .collect()
}

/// Whether a URL goes only to this machine.
pub(crate) fn local_url(url: &str) -> bool {
    ["http://127.0.0.1", "http://localhost"].iter().any(|host| {
        url.strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
    })
}

/// Every value expression in a descriptor.
fn exprs(d: &Descriptor) -> Vec<&Expr> {
    let runs = &d.runs;
    let labels = d.badges.iter().map(|b| &b.label).chain(&d.header);
    [&runs.id, &runs.label, &runs.agent, &runs.status]
        .into_iter()
        .flatten()
        .chain(runs.facts.iter().map(|(_, e)| e))
        .chain(d.rules.iter().filter_map(|r| r.value.as_ref()))
        .chain(labels.flat_map(|l| l.value.iter().chain(l.vars.iter().map(|(_, e)| e))))
        .chain(d.links.iter().filter_map(|l| l.value.as_ref()))
        .chain(d.bind.iter().flat_map(|b| b.process.iter().map(|p| &p.pid)))
        .chain(d.detail.iter().map(|r| &r.list))
        .collect()
}

fn check_expr(expr: &Expr, newer: bool) -> Result<(), LoadError> {
    match expr {
        Expr::Source(source) => check_source(source),
        // A map that isn't a source may be one from a newer minor.
        Expr::Literal(serde_json::Value::Object(map)) if !newer => {
            let why = serde_json::from_value::<Source>(serde_json::Value::Object(map.clone()))
                .err()
                .map_or_else(String::new, |e| format!(": {e}"));
            Err(invalid(format!("A value isn't a value source{why}.")))
        }
        Expr::Literal(_) => Ok(()),
    }
}

fn check_source(s: &Source) -> Result<(), LoadError> {
    let kinds = [
        s.path.is_some(),
        s.count.is_some(),
        s.sum.is_some(),
        s.pattern.is_some(),
        s.alive_pid.is_some(),
    ];
    if kinds.iter().filter(|k| **k).count() != 1 {
        return Err(invalid(
            "A value source has exactly one of `path`, `count`, `sum`, `match` and `alive_pid`.",
        ));
    }
    if s.each.is_some() && s.sum.is_none() {
        return Err(invalid("`each` goes with `sum`."));
    }
    if (s.pick.is_some() || s.group.is_some()) && s.pattern.is_none() {
        return Err(invalid("`pick` and `group` go with `match`."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_that_expands_too_far_is_refused_without_expanding() {
        let list = |item: &str, n: usize| format!("[{}]", vec![item; n].join(","));
        let bomb = format!(
            "format: maxhq.workflow/1.0\na: &a {}\nb: &b {}\nc: {}\n",
            list("1", 2500),
            list("*a", 610),
            list("*b", 610)
        );
        assert!(bomb.len() < 9 * 1024);
        assert_eq!(
            load(bomb.as_bytes()).unwrap_err(),
            LoadError::Parse("It expands to too much.".into())
        );
        assert_eq!(crate::eval::parse_doc(bomb.as_bytes()), None);
    }

    #[test]
    fn flow_too_deep_is_refused_before_the_scanner_sees_it() {
        let deep = format!("x: {}\n", "[".repeat(40_000));
        let started = std::time::Instant::now();
        assert!(matches!(load(deep.as_bytes()), Err(LoadError::Parse(_))));
        assert_eq!(crate::eval::parse_doc(deep.as_bytes()), None);
        // The scanner takes seconds over this, in a release build too.
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_descriptor_past_the_limit_is_refused() {
        let mut text = minimal("");
        text.push_str(&"# padding\n".repeat(MAX_DESCRIPTOR / 10 + 1));
        assert_eq!(
            load(text.as_bytes()).unwrap_err(),
            LoadError::Parse("The file is too large.".into())
        );
    }

    #[test]
    fn a_descriptors_flow_nesting_and_aliases_still_load() {
        let text = minimal("name: &n Example\nsummary: *n\nrevision: 3\n");
        assert!(read(text.as_bytes()).is_ok());
    }

    fn minimal(extra: &str) -> String {
        format!(
            "format: maxhq.workflow/1.0\nid: example\nruns: {{ glob: \"{{project}}/runs/*/run.yaml\" }}\n{extra}"
        )
    }

    fn nested(levels: usize) -> String {
        let mut when = "{ node: a }".to_owned();
        for _ in 1..levels {
            when = format!("{{ not: {when} }}");
        }
        when
    }

    #[test]
    fn a_predicate_nested_too_deep_is_refused_wherever_it_is() {
        let places = [
            format!("rules: [{{ when: {}, is: done }}]\n", nested(MAX_DEPTH + 1)),
            format!(
                "rules: [{{ when: {{ node: a }}, provisional: {} }}]\n",
                nested(MAX_DEPTH + 1)
            ),
            format!("header: [{{ text: a, when: {} }}]\n", nested(MAX_DEPTH + 1)),
            format!("badges: [{{ text: a, when: {} }}]\n", nested(MAX_DEPTH + 1)),
            format!(
                "links: [{{ label: a, url: 'http://localhost/', when: {} }}]\n",
                nested(MAX_DEPTH + 1)
            ),
        ];
        for place in places {
            let Err(LoadError::Invalid(why)) = load(minimal(&place).as_bytes()) else {
                panic!("{place} loaded");
            };
            assert!(why.contains("levels deep"), "{why}");
        }
        let fits = format!("rules: [{{ when: {}, is: done }}]\n", nested(MAX_DEPTH));
        assert!(load(minimal(&fits).as_bytes()).is_ok());
    }

    #[test]
    fn a_minimal_descriptor_loads_with_the_reference_shape() {
        let d = load(minimal("").as_bytes()).unwrap();
        assert_eq!(d.id, "example");
        assert_eq!(d.name(), "example");
        assert_eq!(d.nodes.id, "id");
        assert!(!d.newer());
    }

    #[test]
    fn a_newer_minor_loads_and_ignores_unknown_keys() {
        let text = minimal("future: { anything: 1 }\n").replace("/1.0", "/1.7");
        let d = load(text.as_bytes()).unwrap();
        assert_eq!(d.minor, 7);
        assert!(d.newer());
    }

    #[test]
    fn extension_keys_are_ignored() {
        assert!(load(minimal("x-editor: { color: blue }\n").as_bytes()).is_ok());
    }

    #[test]
    fn a_key_the_standard_lacks_is_refused_by_its_path() {
        let refused = |extra: &str| match load(minimal(extra).as_bytes()) {
            Err(LoadError::Invalid(text)) => text,
            other => panic!("{extra}: {other:?}"),
        };
        assert_eq!(
            refused("future: 1\n"),
            "`future` isn't a key of maxhq.workflow/1.0."
        );
        let link =
            "links: [{ label: Open, url: 'http://127.0.0.1/', when: { alive_pid: a.pid } }]\n";
        assert!(refused(link).starts_with("`links.0.when.alive_pid`"));
        let rule =
            "rules: [{ when: { status: busy }, provisional: { any: [{ stauts: busy }] } }]\n";
        assert!(refused(rule).starts_with("`rules.0.provisional.any.0.stauts`"));
        assert!(refused("badges: [{ text: a, colour: red }]\n").starts_with("`badges.0.colour`"));
        assert!(refused("header: [{ text: a, colour: red }]\n").starts_with("`header.0.colour`"));
        assert!(
            refused("nodes: { info: { a: { x-mine: 1 } } }\n").starts_with("`nodes.info.a.x-mine`")
        );
        let ok = "badges: [{ node: a, text: b, when: { status: busy } }]\nrules: [{ when: {}, provisional: true }]\n";
        assert!(load(minimal(ok).as_bytes()).is_ok());
    }

    #[test]
    fn a_newer_major_does_not_load() {
        let text = minimal("").replace("/1.0", "/2.0");
        assert_eq!(
            load(text.as_bytes()).unwrap_err(),
            LoadError::TooNew {
                id: Some("example".into()),
                major: 2
            }
        );
    }

    #[test]
    fn other_formats_and_missing_versions_are_refused() {
        assert!(matches!(
            load(b"format: other/1.0\nid: x\n"),
            Err(LoadError::Format(_))
        ));
        assert!(matches!(
            load(b"format: maxhq.workflow\nid: x\n"),
            Err(LoadError::Format(_))
        ));
        assert!(matches!(load(b"id: x\n"), Err(LoadError::Format(_))));
        assert!(matches!(load(b": : :"), Err(LoadError::Parse(_))));
    }

    #[test]
    fn env_names_must_start_with_the_id() {
        let ok = minimal("").replace("runs: {", "runs: { env: { EXAMPLE_DIAG: \"1\" },");
        assert_eq!(
            load(ok.as_bytes())
                .unwrap()
                .runs
                .env
                .get("EXAMPLE_DIAG")
                .unwrap(),
            "1"
        );
        for bad in ["PATH", "EXAMPLE_", "EXAMPLEX_A", "EXAMPLE_a"] {
            let text = minimal("").replace("runs: {", &format!("runs: {{ env: {{ {bad}: x }},"));
            assert!(
                matches!(load(text.as_bytes()), Err(LoadError::Invalid(_))),
                "{bad}"
            );
        }
    }

    fn with_env(id: &str, name: &str) -> String {
        minimal("")
            .replace("id: example", &format!("id: {id}"))
            .replace("runs: {", &format!("runs: {{ env: {{ {name}: x }},"))
    }

    /// Whether a descriptor whose id is `id` and that names `name` loads,
    /// and the card that asks for it marks it.
    fn marked(id: &str, name: &str) -> bool {
        load(with_env(id, name).as_bytes()).is_ok() && crate::env_risk(name).is_some()
    }

    #[test]
    fn env_names_in_a_risky_family_load_and_are_marked() {
        for (id, name) in [
            ("anthropic", "ANTHROPIC_BASE_URL"),
            ("claude", "CLAUDE_CONFIG_DIR"),
            ("openai", "OPENAI_BASE_URL"),
            ("node", "NODE_OPTIONS"),
            ("ld", "LD_PRELOAD"),
            ("dyld", "DYLD_INSERT_LIBRARIES"),
            ("git", "GIT_SSH_COMMAND"),
            ("bash", "BASH_ENV"),
            ("ssh", "SSH_ASKPASS"),
            ("gh", "GH_HOST"),
            ("http", "HTTP_PROXY"),
            ("https", "HTTPS_PROXY"),
            ("all", "ALL_PROXY"),
            ("no", "NO_PROXY"),
            ("ssl", "SSL_CERT_FILE"),
            ("pip", "PIP_INDEX_URL"),
            ("npm", "NPM_CONFIG_REGISTRY"),
            ("rustc", "RUSTC_WRAPPER"),
            ("java", "JAVA_TOOL_OPTIONS"),
            ("docker", "DOCKER_HOST"),
            ("xdg", "XDG_CONFIG_HOME"),
            // The id can spell the family and more.
            ("cargo-home", "CARGO_HOME_X"),
            ("cargo.home", "CARGO_HOME_X"),
            ("path", "PATH_X"),
            ("home", "HOME_X"),
            ("all-proxy", "ALL_PROXY_X"),
        ] {
            assert!(marked(id, name), "{id} {name}");
        }
    }

    #[test]
    fn env_names_that_run_code_or_change_a_login_through_a_tool_are_marked() {
        let unmarked: Vec<_> = [
            ("openssl", "OPENSSL_CONF"),
            ("openssl", "OPENSSL_MODULES"),
            ("openssl", "OPENSSL_ENGINES"),
            ("ripgrep", "RIPGREP_CONFIG_PATH"),
            ("developer", "DEVELOPER_DIR"),
            ("tar", "TAR_OPTIONS"),
            ("xz", "XZ_OPT"),
            ("compiler", "COMPILER_PATH"),
            ("gcc", "GCC_EXEC_PREFIX"),
            ("c", "C_INCLUDE_PATH"),
            ("cplus", "CPLUS_INCLUDE_PATH"),
            ("library", "LIBRARY_PATH"),
            ("asan", "ASAN_OPTIONS"),
            ("llvm", "LLVM_SYMBOLIZER_PATH"),
            ("xcode", "XCODE_DEVELOPER_DIR"),
            ("puppeteer", "PUPPETEER_EXECUTABLE_PATH"),
            ("playwright", "PLAYWRIGHT_BROWSERS_PATH"),
            ("cypress", "CYPRESS_RUN_BINARY"),
            ("esbuild", "ESBUILD_BINARY_PATH"),
            ("chrome", "CHROME_BIN"),
            ("chrome", "CHROME_PATH"),
            ("electron", "ELECTRON_RUN_AS_NODE"),
            ("systemd", "SYSTEMD_PAGER"),
            ("fzf", "FZF_DEFAULT_COMMAND"),
            ("bat", "BAT_PAGER"),
            ("delta", "DELTA_PAGER"),
            ("ts-node", "TS_NODE_COMPILER"),
            ("erl", "ERL_FLAGS"),
            ("r", "R_PROFILE_USER"),
            ("pre-commit", "PRE_COMMIT_HOME"),
            ("svn", "SVN_SSH"),
            ("getconf", "GETCONF_DIR"),
            ("cloudsdk", "CLOUDSDK_CONFIG"),
            ("gcloud", "GCLOUD_PROJECT"),
            ("boto", "BOTO_CONFIG"),
            ("rclone", "RCLONE_CONFIG"),
            ("vault", "VAULT_ADDR"),
            ("krb5", "KRB5_CONFIG"),
            ("res", "RES_OPTIONS"),
            ("resolv", "RESOLV_HOST_CONF"),
            ("ws", "WS_PROXY"),
            ("wss", "WSS_PROXY"),
            ("global-agent", "GLOBAL_AGENT_HTTP_PROXY"),
            ("ansible", "ANSIBLE_CONFIG"),
            ("composer", "COMPOSER_HOME"),
            ("nodenv", "NODENV_ROOT"),
            ("goenv", "GOENV_ROOT"),
            ("jenv", "JENV_ROOT"),
            ("em", "EM_CONFIG"),
            ("terragrunt", "TERRAGRUNT_TFPATH"),
            ("tg", "TG_TF_PATH"),
            ("parallel", "PARALLEL_SHELL"),
            ("autossh", "AUTOSSH_PATH"),
            ("android", "ANDROID_HOME"),
            ("otel", "OTEL_EXPORTER_OTLP_ENDPOINT"),
            ("otel", "OTEL_EXPORTER_OTLP_HEADERS"),
            ("pulumi", "PULUMI_ACCESS_TOKEN"),
            ("op", "OP_CONNECT_HOST"),
            ("app-store", "APP_STORE_CONNECT_API_KEY_PATH"),
        ]
        .into_iter()
        .filter(|(id, name)| !marked(id, name))
        .collect();
        assert!(unmarked.is_empty(), "{unmarked:?}");
    }

    #[test]
    fn env_names_outside_the_risky_families_load_unmarked() {
        for (id, name) in [
            ("devflow", "DEVFLOW_AGENT_TYPES"),
            ("devflow", "DEVFLOW_CHILD"),
            ("devflow", "DEVFLOW_DIAG"),
            ("devflow", "DEVFLOW_DIAG_HOME"),
            ("devflow", "DEVFLOW_HOST"),
            ("devflow", "DEVFLOW_MEMORY_HOME"),
            ("devflow", "DEVFLOW_ORIGIN"),
            ("devflow", "DEVFLOW_PY"),
            ("devflow", "DEVFLOW_SKIP_GRAPH"),
            ("devflow", "DEVFLOW_SKIP_PULL"),
            ("devflow", "DEVFLOW_UI_HOME"),
            ("devflow", "DEVFLOW_VENDOR"),
            ("pipeline", "PIPELINE_TRACE"),
            ("checks", "CHECKS_MODE"),
            ("release", "RELEASE_MODE"),
            ("example", "EXAMPLE_DIAG"),
            // A family ends at its `_`, so these start with one but aren't in it.
            ("gitmoji", "GITMOJI_X"),
            ("notes", "NOTES_X"),
            ("allow", "ALLOW_X"),
            ("good", "GOOD_X"),
            ("terminal", "TERMINAL_X"),
            // Only the name that matters is kept from these words.
            ("developer", "DEVELOPER_NOTES"),
            ("delta", "DELTA_TRACE"),
            ("coverage", "COVERAGE_TRACE"),
            ("res", "RES_MODE"),
            ("ws", "WS_MODE"),
            ("c", "C_MODE"),
            ("r", "R_MODE"),
            ("all-tests", "ALL_TESTS_X"),
            ("no-regressions", "NO_REGRESSIONS_X"),
            ("op", "OP_MODE"),
            ("em", "EM_MODE"),
            ("tg", "TG_MODE"),
            ("parallel", "PARALLEL_JOBS"),
        ] {
            let loaded = load(with_env(id, name).as_bytes());
            assert!(loaded.is_ok(), "{id} {name}: {loaded:?}");
            assert_eq!(crate::env_risk(name), None, "{id} {name}");
        }
    }

    #[test]
    fn the_env_prefix_comes_from_the_id() {
        let mut d = Descriptor::agents();
        d.id = "my-flow.v2".into();
        assert_eq!(d.env_prefix(), "MY_FLOW_V2_");
    }

    #[test]
    fn globs_must_stay_in_the_project() {
        for glob in ["/etc/*.yaml", "../other/*.yaml", "{project}/../x.yaml", ""] {
            let text = minimal("").replace("{project}/runs/*/run.yaml", glob);
            assert!(
                matches!(load(text.as_bytes()), Err(LoadError::Invalid(_))),
                "{glob}"
            );
        }
    }

    #[test]
    fn links_go_only_to_this_machine() {
        let link = |url: &str| minimal(&format!("links: [{{ label: Open, url: \"{url}\" }}]\n"));
        assert!(load(link("http://127.0.0.1:{value}/").as_bytes()).is_ok());
        assert!(load(link("http://localhost/x").as_bytes()).is_ok());
        for url in [
            "https://example.com",
            "http://127.0.0.1.example.com/",
            "file:///etc",
        ] {
            assert!(load(link(url).as_bytes()).is_err(), "{url}");
        }
    }

    #[test]
    fn a_value_source_has_exactly_one_kind() {
        let fact = |source: &str| {
            minimal("").replace("runs: {", &format!("runs: {{ facts: {{ f: {source} }},"))
        };
        assert!(load(fact("{ path: $.a }").as_bytes()).is_ok());
        assert!(load(fact("{ path: $.a, count: $.b }").as_bytes()).is_err());
        assert!(load(fact("{ path: $.a, each: { add: 1 } }").as_bytes()).is_err());
        assert!(load(fact("{ path: $.a, pick: last }").as_bytes()).is_err());
        assert!(load(fact("{ paht: $.a }").as_bytes()).is_err());
        assert!(load(fact("{ path: \"not a path\" }").as_bytes()).is_err());
        assert!(load(fact("12").as_bytes()).is_ok());
    }

    #[test]
    fn overlays_merge_over_the_descriptor() {
        let Raw::Workflow { mut value, .. } =
            read(minimal("name: Example\nrevision: 3\n").as_bytes()).unwrap()
        else {
            panic!("a workflow")
        };
        let Raw::Overlay { workflow, value: patch } = read(
            b"format: maxhq.workflow-overlay/1.0\nworkflow: example\nname: Mine\nrevision: null\ngroups: { a: { label: A } }\n",
        )
        .unwrap() else {
            panic!("an overlay")
        };
        assert_eq!(workflow, "example");
        merge(&mut value, patch);
        let d = finish(value, 0).unwrap();
        assert_eq!(d.name(), "Mine");
        assert_eq!(d.revision, None);
        assert_eq!(d.groups.get("a").unwrap().label.as_deref(), Some("A"));
        assert_eq!(d.format, "maxhq.workflow/1.0");
    }

    #[test]
    fn maps_keep_their_order() {
        let d = load(minimal("groups: { z: {}, a: {}, m: {} }\n").as_bytes()).unwrap();
        let ids: Vec<_> = d.groups.iter().map(|(k, _)| k).collect();
        assert_eq!(ids, ["z", "a", "m"]);
    }

    #[test]
    fn reserved_ids_are_refused() {
        let text = minimal("").replace("id: example", "id: maxhq.agents");
        assert!(load(text.as_bytes()).is_err());
        let text = minimal("").replace("id: example", "id: Example");
        assert!(load(text.as_bytes()).is_err());
    }
}
