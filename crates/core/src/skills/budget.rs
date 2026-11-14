// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How much of each CLI's skill listing a set takes. Claude lists skills in
//! 1% of the model's context and drops the least used descriptions past
//! it; Codex takes 2%, or 8,000 characters when it doesn't know the
//! context. Cursor and ACP agents don't say, so they get no limit.

use maxhq_protocol::model::Vendor;
use maxhq_protocol::skills::EffectiveSkill;

use super::frontmatter::CHARS_PER_TOKEN;

/// The most characters of a description Claude lists.
pub const MAX_ENTRY_DESCRIPTION: usize = 1536;

/// The listing's size when a CLI doesn't know the model's context, in
/// characters.
pub const FALLBACK_CHARS: u32 = 8_000;

/// The characters around each listed entry: `- `, `: ` and a line break.
const ENTRY_OVERHEAD: usize = 5;

/// About how many characters one skill adds to a listing.
pub fn entry_chars(name: &str, description: Option<&str>) -> u32 {
    let description = description.map_or(0, |d| d.chars().count().min(MAX_ENTRY_DESCRIPTION));
    let chars = name.chars().count() + description + ENTRY_OVERHEAD;
    u32::try_from(chars).unwrap_or(u32::MAX)
}

/// The share of the context a CLI lists skills in, in percent, when it
/// says.
pub fn percent(cli: Vendor) -> Option<u32> {
    match cli {
        Vendor::Claude => Some(1),
        Vendor::Codex => Some(2),
        Vendor::Cursor | Vendor::Acp => None,
    }
}

/// The listing's size for `cli` with a model of `context_tokens`, in
/// characters, when the CLI has one.
pub fn limit(cli: Vendor, context_tokens: Option<u32>) -> Option<u32> {
    let percent = percent(cli)?;
    Some(context_tokens.map_or(FALLBACK_CHARS, |tokens| {
        let chars = u64::from(tokens) * CHARS_PER_TOKEN as u64 * u64::from(percent) / 100;
        u32::try_from(chars).unwrap_or(u32::MAX)
    }))
}

/// One CLI's listing against its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// The CLI.
    pub cli: Vendor,
    /// The skills it lists.
    pub listed: u32,
    /// About how many characters they take.
    pub used: u32,
    /// The listing's size, when the CLI has one.
    pub limit: Option<u32>,
}

impl Budget {
    /// Whether the listing is past its size, so the CLI drops descriptions.
    pub fn over(&self) -> bool {
        self.limit.is_some_and(|limit| self.used > limit)
    }

    /// Whether going from `self` to `after` pushes descriptions out, the
    /// case to warn about before an apply.
    pub fn pushed_out_by(&self, after: &Budget) -> bool {
        !self.over() && after.over()
    }
}

/// What the skills `cli` sees take of its listing. Only skills it offers
/// and the model may call itself are listed.
pub fn estimate(cli: Vendor, skills: &[EffectiveSkill], context_tokens: Option<u32>) -> Budget {
    let mut budget = Budget {
        cli,
        listed: 0,
        used: 0,
        limit: limit(cli, context_tokens),
    };
    for skill in skills {
        if skill.cli != cli || !skill.enabled || skill.user_invocation_only == Some(true) {
            continue;
        }
        budget.listed += 1;
        let entry = entry_chars(&skill.name, skill.description.as_deref());
        budget.used = budget.used.saturating_add(entry);
    }
    budget
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::skills::SkillPlace;

    use super::*;

    fn skill(cli: Vendor, name: &str, description: &str) -> EffectiveSkill {
        EffectiveSkill {
            name: name.into(),
            description: Some(description.into()),
            path: format!("/work/repo/.claude/skills/{name}/SKILL.md"),
            scope: None,
            enabled: true,
            display_name: None,
            short_description: None,
            user_invocation_only: None,
            user_invocable: None,
            place: SkillPlace::Project,
            cli,
            mark: None,
        }
    }

    #[test]
    fn an_entry_caps_its_description() {
        assert_eq!(entry_chars("ab", Some("abcd")), 2 + 4 + 5);
        assert_eq!(entry_chars("ab", None), 7);
        assert_eq!(
            entry_chars("a", Some(&"x".repeat(5000))),
            1 + MAX_ENTRY_DESCRIPTION as u32 + 5
        );
    }

    #[test]
    fn limits_follow_each_cli() {
        assert_eq!(limit(Vendor::Claude, Some(200_000)), Some(8_000));
        assert_eq!(limit(Vendor::Codex, Some(200_000)), Some(16_000));
        assert_eq!(limit(Vendor::Codex, None), Some(FALLBACK_CHARS));
        assert_eq!(limit(Vendor::Cursor, Some(200_000)), None);
    }

    #[test]
    fn only_listed_skills_of_the_cli_count() {
        let mut hidden = skill(Vendor::Claude, "manual", "only you");
        hidden.user_invocation_only = Some(true);
        let mut off = skill(Vendor::Claude, "off", "shadowed");
        off.enabled = false;
        let skills = [
            skill(Vendor::Claude, "a", "abcd"),
            skill(Vendor::Codex, "a", "abcd"),
            hidden,
            off,
        ];
        let budget = estimate(Vendor::Claude, &skills, Some(1_000));
        assert_eq!(budget.listed, 1);
        assert_eq!(budget.used, 10);
        assert_eq!(budget.limit, Some(40));
        assert!(!budget.over());
    }

    #[test]
    fn a_growing_set_warns_once_it_passes_the_limit() {
        let small = [skill(Vendor::Claude, "a", &"x".repeat(30))];
        let large = [
            skill(Vendor::Claude, "a", &"x".repeat(30)),
            skill(Vendor::Claude, "b", &"x".repeat(30)),
        ];
        let before = estimate(Vendor::Claude, &small, Some(1_000));
        let after = estimate(Vendor::Claude, &large, Some(1_000));
        assert!(before.pushed_out_by(&after));
        assert!(!after.pushed_out_by(&after));
        assert!(!estimate(Vendor::Cursor, &large, None).over());
    }
}
