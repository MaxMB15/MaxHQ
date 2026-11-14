// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `SKILL.md` and Codex's `agents/openai.yaml`: reading them, checking a
//! skill against the Agent Skills spec, flagging what it does beyond its
//! instructions, and estimating what it costs.

use maxhq_protocol::message::{EntryKind, ManifestEntry};
use maxhq_protocol::skills::SkillFlags;
use serde_norway::Value;
use thiserror::Error;

use super::budget;

/// The longest name the spec allows, in characters.
pub const MAX_NAME: usize = 64;

/// The longest description the spec allows, in characters.
pub const MAX_DESCRIPTION: usize = 1024;

/// The longest `compatibility` the spec allows, in characters.
pub const MAX_COMPATIBILITY: usize = 500;

/// The fields the Agent Skills spec defines. Anything else, such as
/// Claude's `disable-model-invocation`, works in some CLIs only.
pub const SPEC_FIELDS: [&str; 6] = [
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// About how many characters one token is, for estimates.
pub const CHARS_PER_TOKEN: usize = 4;

/// A `SKILL.md`, split into its fields and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDoc {
    /// The frontmatter's fields, in file order. Empty when there's no
    /// frontmatter.
    pub fields: Vec<(String, Value)>,
    /// Whether the file starts with frontmatter.
    pub has_frontmatter: bool,
    /// Everything after the frontmatter.
    pub body: String,
}

/// Why a `SKILL.md` or `agents/openai.yaml` can't be read.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ParseError {
    /// The frontmatter's opening `---` has no closing one.
    #[error("The frontmatter never closes.")]
    Unclosed,
    /// The YAML doesn't parse, with the parser's reason.
    #[error("The YAML isn't valid: {0}")]
    Yaml(String),
    /// The YAML is a list or a scalar, not fields.
    #[error("The YAML isn't a list of fields.")]
    NotMap,
}

/// The most text a frontmatter or `agents/openai.yaml` may be. Real ones are
/// a few hundred bytes.
const MAX_YAML: usize = 64 * 1024;

/// The most nodes such a text may expand to, aliases counted each time they
/// repeat.
const MAX_NODES: u64 = 100_000;

/// The fields of a YAML map, in file order. An empty file has none; keys
/// that aren't text are left out, since no CLI reads them.
fn fields(text: &str) -> Result<Vec<(String, Value)>, ParseError> {
    if text.len() > MAX_YAML {
        return Err(ParseError::Yaml("It's too large.".to_owned()));
    }
    let document = serde_norway::Deserializer::from_str(text);
    maxhq_protocol::yaml::check(text.as_bytes(), document, MAX_NODES)
        .map_err(|why| ParseError::Yaml(why.to_owned()))?;
    let value: Value =
        serde_norway::from_str(text).map_err(|err| ParseError::Yaml(err.to_string()))?;
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Mapping(map) => Ok(map
            .into_iter()
            .filter_map(|(key, value)| Some((key.as_str()?.to_owned(), value)))
            .collect()),
        _ => Err(ParseError::NotMap),
    }
}

/// What [`check`] finds wrong with a skill, or worth knowing.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum Problem {
    /// No frontmatter, so no CLI but Claude reads it as a skill.
    #[error("It has no frontmatter.")]
    NoFrontmatter,
    /// No `name`.
    #[error("It has no name.")]
    NoName,
    /// A name breaking the spec's rules.
    #[error("Its name {0}.")]
    BadName(NameProblem),
    /// A name that isn't its folder's.
    #[error("Its name isn't its folder's, {folder}.")]
    NameNotFolder {
        /// The folder's name.
        folder: String,
    },
    /// No `description`, or an empty one.
    #[error("It has no description.")]
    NoDescription,
    /// A description over [`MAX_DESCRIPTION`] characters.
    #[error("Its description is over {MAX_DESCRIPTION} characters.")]
    DescriptionTooLong,
    /// A `compatibility` over [`MAX_COMPATIBILITY`] characters.
    #[error("Its compatibility note is over {MAX_COMPATIBILITY} characters.")]
    CompatibilityTooLong,
    /// A spec field that should be text and isn't.
    #[error("Its {0} isn't text.")]
    NotText(String),
    /// A field outside the spec, which only some CLIs read.
    #[error("Its {0} field isn't in the Agent Skills spec, so only some CLIs read it.")]
    NotInSpec(String),
}

impl Problem {
    /// Whether the skill breaks the spec, rather than only using a field
    /// some CLIs ignore.
    pub fn is_error(&self) -> bool {
        !matches!(self, Problem::NotInSpec(_))
    }
}

/// How a name breaks the spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum NameProblem {
    /// Empty.
    #[error("is empty")]
    Empty,
    /// Over [`MAX_NAME`] characters.
    #[error("is over {MAX_NAME} characters")]
    TooLong,
    /// Something other than lowercase letters, digits and hyphens.
    #[error("has characters other than lowercase letters, digits and hyphens")]
    Chars,
    /// A hyphen first, last or twice in a row.
    #[error("starts or ends with a hyphen, or has two in a row")]
    Hyphens,
}

/// Checks a skill name against the spec: 1 to 64 lowercase letters,
/// digits and hyphens, with no hyphen first, last or doubled.
pub fn check_name(name: &str) -> Result<(), NameProblem> {
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    if name.chars().count() > MAX_NAME {
        return Err(NameProblem::TooLong);
    }
    if !name
        .chars()
        .all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(NameProblem::Chars);
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(NameProblem::Hyphens);
    }
    Ok(())
}

/// Reads a `SKILL.md`. A file with no frontmatter reads as all body.
pub fn parse(text: &str) -> Result<SkillDoc, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(after) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Ok(SkillDoc {
            fields: Vec::new(),
            has_frontmatter: false,
            body: text.to_owned(),
        });
    };
    let mut offset = 0;
    let mut end = None;
    for line in after.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            end = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let (yaml_end, body_start) = end.ok_or(ParseError::Unclosed)?;
    Ok(SkillDoc {
        fields: fields(&after[..yaml_end])?,
        has_frontmatter: true,
        body: after[body_start..].to_owned(),
    })
}

impl SkillDoc {
    /// The field `key`, when it's there.
    pub fn field(&self, key: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The field `key` as trimmed text, when it's there and not empty.
    pub fn text(&self, key: &str) -> Option<&str> {
        let text = self.field(key)?.as_str()?.trim();
        (!text.is_empty()).then_some(text)
    }

    /// The `name` field.
    pub fn name(&self) -> Option<&str> {
        self.text("name")
    }

    /// The `description` field.
    pub fn description(&self) -> Option<&str> {
        self.text("description")
    }

    /// Claude's `disable-model-invocation: true`: only you can call it.
    pub fn user_invocation_only(&self) -> bool {
        self.field("disable-model-invocation")
            .and_then(Value::as_bool)
            == Some(true)
    }

    /// Claude's `user-invocable: false`: only agents can call it.
    pub fn user_invocable(&self) -> bool {
        self.field("user-invocable").and_then(Value::as_bool) != Some(false)
    }

    /// Whether a CLI is among `metadata.surfaces`, when the skill names
    /// any. Cursor hides a skill whose surfaces leave out `cli`.
    pub fn on_surface(&self, surface: &str) -> bool {
        let surfaces: Vec<&str> = match self.field("metadata").and_then(|m| m.get("surfaces")) {
            Some(Value::Sequence(items)) => items.iter().filter_map(Value::as_str).collect(),
            Some(Value::String(text)) => text.split(',').collect(),
            _ => Vec::new(),
        };
        surfaces.is_empty()
            || surfaces
                .iter()
                .any(|s| s.trim().eq_ignore_ascii_case(surface))
    }

    /// About how many tokens the body costs once the skill is invoked.
    pub fn body_tokens(&self) -> u32 {
        let chars = self.body.chars().count();
        u32::try_from(chars.div_ceil(CHARS_PER_TOKEN)).unwrap_or(u32::MAX)
    }

    /// About how many characters the skill adds to a CLI's listing, named
    /// `name`.
    pub fn listing_chars(&self, name: &str) -> u32 {
        budget::entry_chars(name, self.description())
    }
}

/// Checks a skill in the folder `folder` against the spec, and notes each
/// field outside it.
pub fn check(doc: &SkillDoc, folder: &str) -> Vec<Problem> {
    let mut problems = Vec::new();
    if !doc.has_frontmatter {
        problems.push(Problem::NoFrontmatter);
    }
    for key in ["name", "description", "license", "compatibility"] {
        if doc.field(key).is_some_and(|v| v.as_str().is_none()) {
            problems.push(Problem::NotText(key.to_owned()));
        }
    }
    match doc.name() {
        None => problems.push(Problem::NoName),
        Some(name) => {
            if let Err(problem) = check_name(name) {
                problems.push(Problem::BadName(problem));
            }
            if name != folder {
                problems.push(Problem::NameNotFolder {
                    folder: folder.to_owned(),
                });
            }
        }
    }
    match doc.description() {
        None => problems.push(Problem::NoDescription),
        Some(text) if text.chars().count() > MAX_DESCRIPTION => {
            problems.push(Problem::DescriptionTooLong);
        }
        Some(_) => {}
    }
    if doc
        .text("compatibility")
        .is_some_and(|text| text.chars().count() > MAX_COMPATIBILITY)
    {
        problems.push(Problem::CompatibilityTooLong);
    }
    for (key, _) in &doc.fields {
        if !SPEC_FIELDS.contains(&key.as_str()) {
            problems.push(Problem::NotInSpec(key.clone()));
        }
    }
    problems
}

/// What a skill does beyond its instructions: scripts, which are any
/// executable file or anything under `scripts/`, hooks, and
/// `allowed-tools`.
pub fn flags(doc: &SkillDoc, files: &[ManifestEntry]) -> SkillFlags {
    SkillFlags {
        scripts: files.iter().any(|entry| {
            entry.kind == EntryKind::File
                && (entry.executable || entry.path.starts_with("scripts/"))
        }),
        hooks: doc.field("hooks").is_some(),
        allowed_tools: doc.field("allowed-tools").is_some(),
    }
}

/// Where Codex reads a skill's extra metadata, from the skill's folder.
pub const CODEX_META: &str = "agents/openai.yaml";

/// What Codex's `agents/openai.yaml` adds to a skill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexMeta {
    /// `interface.display_name`.
    pub display_name: Option<String>,
    /// `interface.short_description`.
    pub short_description: Option<String>,
    /// `policy.allow_implicit_invocation`; when false Codex uses the skill
    /// only when you name it.
    pub allow_implicit_invocation: Option<bool>,
}

/// Reads Codex's `agents/openai.yaml`.
pub fn codex_meta(text: &str) -> Result<CodexMeta, ParseError> {
    let doc = fields(text)?;
    let get = |section: &str, key: &str| {
        let (_, value) = doc.iter().find(|(k, _)| k == section)?;
        value.get(key)
    };
    let text = |section: &str, key: &str| {
        let value = get(section, key)?.as_str()?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    };
    Ok(CodexMeta {
        display_name: text("interface", "display_name"),
        short_description: text("interface", "short_description"),
        allow_implicit_invocation: get("policy", "allow_implicit_invocation")
            .and_then(Value::as_bool),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Under nine kilobytes of nested aliases, for 930 million values.
    fn alias_bomb() -> String {
        let list = |item: &str, n: usize| format!("[{}]", vec![item; n].join(","));
        format!(
            "a: &a {}\nb: &b {}\nc: {}\n",
            list("1", 2500),
            list("*a", 610),
            list("*b", 610)
        )
    }

    #[test]
    fn yaml_that_expands_too_far_is_refused_without_expanding() {
        let bomb = alias_bomb();
        assert!(bomb.len() < 9 * 1024);
        assert!(matches!(fields(&bomb), Err(ParseError::Yaml(_))));
        assert!(matches!(codex_meta(&bomb), Err(ParseError::Yaml(_))));
        assert!(parse(&format!("---\n{bomb}---\n\nbody\n")).is_err());
    }

    #[test]
    fn a_long_text_repeated_by_aliases_is_refused() {
        let text = "x".repeat(30_000);
        let aliases = vec!["*a"; 8000].join(",");
        let long = format!("a: &a {text}\nb: [{aliases}]\n");
        assert!(matches!(fields(&long), Err(ParseError::Yaml(_))));
    }

    #[test]
    fn a_tagged_node_doesnt_hide_what_it_expands_to() {
        let list = |item: &str, n: usize| format!("[{}]", vec![item; n].join(","));
        let bomb = format!("a: &a {}\nb: !!x {}\n", list("1", 2500), list("*a", 1000),);
        assert!(matches!(fields(&bomb), Err(ParseError::Yaml(_))));
    }

    #[test]
    fn flow_too_deep_is_refused_and_too_much_text_too() {
        let deep = format!("a: {}\n", "[".repeat(40_000));
        assert!(matches!(fields(&deep), Err(ParseError::Yaml(_))));
        let long = format!("a: {}\n", "x".repeat(MAX_YAML));
        assert!(matches!(fields(&long), Err(ParseError::Yaml(_))));
    }

    #[test]
    fn aliases_and_tags_in_ordinary_amounts_still_read() {
        let text = "a: &a [1, 2]\nb: *a\nc: !!str 3\n";
        let read = fields(text).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[1].1, read[0].1);
    }

    fn doc(frontmatter: &str) -> SkillDoc {
        parse(&format!("---\n{frontmatter}---\n# Body\n")).unwrap()
    }

    fn file(path: &str, executable: bool) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::File,
            executable,
            size: 1,
            sha256: Some("00".into()),
        }
    }

    #[test]
    fn a_skill_splits_into_fields_and_body() {
        let skill =
            parse("---\nname: unslop\ndescription: Cut AI tells.\n---\n# Unslop\n").unwrap();
        assert_eq!(skill.name(), Some("unslop"));
        assert_eq!(skill.description(), Some("Cut AI tells."));
        assert_eq!(skill.body, "# Unslop\n");
        assert!(check(&skill, "unslop").is_empty());
    }

    #[test]
    fn a_file_with_no_frontmatter_is_all_body() {
        let skill = parse("# Just text\n").unwrap();
        assert!(!skill.has_frontmatter);
        assert_eq!(skill.body, "# Just text\n");
        let problems = check(&skill, "x");
        assert!(problems.contains(&Problem::NoFrontmatter));
        assert!(problems.contains(&Problem::NoName));
    }

    #[test]
    fn broken_frontmatter_is_an_error() {
        assert_eq!(parse("---\nname: x\n"), Err(ParseError::Unclosed));
        assert!(matches!(
            parse("---\ndescription: Use when: writing\n---\n"),
            Err(ParseError::Yaml(_))
        ));
        assert!(matches!(
            parse("---\nname: a\nname: b\n---\n"),
            Err(ParseError::Yaml(_))
        ));
        assert_eq!(parse("---\n- a\n---\n"), Err(ParseError::NotMap));
        assert_eq!(parse("---\n---\nBody\n").unwrap().fields, Vec::new());
    }

    #[test]
    fn names_follow_the_spec() {
        assert_eq!(check_name("git-flow"), Ok(()));
        assert_eq!(check_name(""), Err(NameProblem::Empty));
        assert_eq!(check_name(&"a".repeat(65)), Err(NameProblem::TooLong));
        assert_eq!(check_name(&"a".repeat(64)), Ok(()));
        assert_eq!(check_name("Git_Flow"), Err(NameProblem::Chars));
        assert_eq!(check_name("-x"), Err(NameProblem::Hyphens));
        assert_eq!(check_name("a--b"), Err(NameProblem::Hyphens));
    }

    #[test]
    fn a_name_must_match_its_folder() {
        let problems = check(&doc("name: unslop\ndescription: d\n"), "deslop");
        assert_eq!(
            problems,
            vec![Problem::NameNotFolder {
                folder: "deslop".into()
            }]
        );
    }

    #[test]
    fn a_description_over_1024_characters_is_flagged() {
        let at = doc(&format!("name: x\ndescription: {}\n", "é".repeat(1024)));
        assert!(check(&at, "x").is_empty());
        let over = doc(&format!("name: x\ndescription: {}\n", "é".repeat(1025)));
        assert_eq!(check(&over, "x"), vec![Problem::DescriptionTooLong]);
        assert_eq!(
            check(&doc("name: x\ndescription: ''\n"), "x"),
            vec![Problem::NoDescription]
        );
    }

    #[test]
    fn claude_only_fields_are_noted_for_the_spec() {
        let skill = doc(concat!(
            "name: x\ndescription: d\nlicense: MIT\n",
            "disable-model-invocation: true\nuser-invocable: false\n",
        ));
        let problems = check(&skill, "x");
        assert_eq!(
            problems,
            vec![
                Problem::NotInSpec("disable-model-invocation".into()),
                Problem::NotInSpec("user-invocable".into()),
            ]
        );
        assert!(problems.iter().all(|p| !p.is_error()));
        assert!(skill.user_invocation_only());
        assert!(!skill.user_invocable());
    }

    #[test]
    fn scripts_hooks_and_allowed_tools_are_flagged() {
        let plain = doc("name: x\ndescription: d\n");
        assert_eq!(
            flags(&plain, &[file("SKILL.md", false)]),
            SkillFlags::default()
        );
        let busy = doc(concat!(
            "name: x\ndescription: d\nallowed-tools: Bash(git *)\n",
            "hooks:\n  PostToolUse:\n    - matcher: Edit\n",
        ));
        let flagged = flags(&busy, &[file("scripts/run.py", false)]);
        assert_eq!(
            flagged,
            SkillFlags {
                scripts: true,
                hooks: true,
                allowed_tools: true
            }
        );
        assert!(flags(&plain, &[file("bin/tool", true)]).scripts);
    }

    #[test]
    fn surfaces_hide_a_skill_from_the_cli() {
        assert!(doc("name: x\n").on_surface("cli"));
        assert!(doc("name: x\nmetadata:\n  surfaces: [CLI, app]\n").on_surface("cli"));
        assert!(!doc("name: x\nmetadata:\n  surfaces: app, web\n").on_surface("cli"));
    }

    #[test]
    fn estimates_count_the_listing_and_the_body() {
        let skill = parse("---\nname: x\ndescription: abcd\n---\n12345678").unwrap();
        assert_eq!(skill.body_tokens(), 2);
        assert_eq!(
            skill.listing_chars("x"),
            budget::entry_chars("x", Some("abcd"))
        );
    }

    #[test]
    fn codex_metadata_reads_its_interface_and_policy() {
        let meta = codex_meta(concat!(
            "interface:\n  display_name: \"Unslop\"\n  short_description: Cut tells\n",
            "policy:\n  allow_implicit_invocation: false\n",
        ))
        .unwrap();
        assert_eq!(
            meta,
            CodexMeta {
                display_name: Some("Unslop".into()),
                short_description: Some("Cut tells".into()),
                allow_implicit_invocation: Some(false),
            }
        );
        assert_eq!(codex_meta("").unwrap(), CodexMeta::default());
        assert!(matches!(
            codex_meta("interface: [\n"),
            Err(ParseError::Yaml(_))
        ));
        assert_eq!(codex_meta("- a\n"), Err(ParseError::NotMap));
    }
}
