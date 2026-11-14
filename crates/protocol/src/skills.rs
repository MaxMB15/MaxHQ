// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Skills. One device's `maxd` holds the library: the sources skills come
//! from, as a [`SkillLibrary`]. Each project keeps a lock of the skills it
//! uses, published as its [`ProjectSkills`], and every device keeps a store
//! of the trees its projects need, named by [`TreeHash`]. Agents propose
//! changes as [`SkillProposal`]s, which you approve or reject.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, DeviceId, ProjectId, SkillProposalId, SkillSourceId};
use crate::model::Vendor;
use crate::pulls::PullFile;
use crate::time::Timestamp;

/// A skill folder's content hash: hex SHA-256 over its sorted files, each
/// with its path, executable bit and hash. The same files always give the
/// same hash.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type, JsonSchema,
)]
#[serde(transparent)]
pub struct TreeHash(pub String);

/// Where a source's skills come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// A git repo the holder clones.
    Git {
        /// What it clones from.
        url: String,
    },
    /// A folder on the holder.
    Local {
        /// Absolute path on the holder.
        path: String,
    },
}

/// Whose a source is, which decides how its updates land.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceTrust {
    /// Yours. Its skills follow updates unless you pin them.
    Own,
    /// Someone else's. Its skills are pinned, and an update shows what it
    /// adds before it lands.
    ThirdParty,
}

/// What a skill does beyond its instructions, which the app flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillFlags {
    /// It ships scripts.
    pub scripts: bool,
    /// It sets hooks.
    pub hooks: bool,
    /// It names `allowed-tools`.
    pub allowed_tools: bool,
}

/// One skill in a source, at the source's commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillInfo {
    /// Its name, which matches its folder.
    pub name: String,
    /// Its description, from `SKILL.md`.
    pub description: String,
    /// Its folder's hash.
    pub tree: TreeHash,
    /// Its folder's path in the source.
    pub path: String,
    /// The source it's in.
    pub source: SkillSourceId,
    /// What it does beyond its instructions.
    pub flags: SkillFlags,
    /// About how many characters it adds to each CLI's skill listing.
    pub listing_chars: u32,
}

/// One source of skills, as the holder keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillSource {
    /// The source.
    pub id: SkillSourceId,
    /// What the app calls it.
    pub name: String,
    /// Where its skills come from.
    pub kind: SourceKind,
    /// The branch or tag it fetches, none for a git repo's default.
    pub default_ref: Option<String>,
    /// Whose it is.
    pub trust: SourceTrust,
    /// The commit its skills are read at, once fetched. None for a local
    /// folder.
    pub commit: Option<String>,
    /// When it was last fetched.
    pub fetched_at: Option<Timestamp>,
    /// Why the last fetch failed, while it has.
    pub problem: Option<String>,
    /// Its skills.
    pub skills: Vec<SkillInfo>,
    /// Its whole tree at `commit`, git's files only, when it's a git repo
    /// with an `install.sh` at its root a project can use as its devflow
    /// pack. A `maxd` from before it leaves this out.
    #[serde(default)]
    pub pack: Option<TreeHash>,
    /// Whether it's a git source with commits its remote doesn't have. A
    /// `maxd` from before it leaves this out.
    #[serde(default)]
    pub unpushed: bool,
}

/// The library, as the device that holds it publishes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillLibrary {
    /// The device that holds it.
    pub device: DeviceId,
    /// Its sources.
    pub sources: Vec<SkillSource>,
    /// The skills every agent gets globally, which project skills can't be
    /// named after.
    pub always: Vec<String>,
    /// The library's skill for each of `always` it keeps, which the app
    /// brings to every other device. One MaxHQ ships isn't here. A `maxd`
    /// from before it leaves this out.
    #[serde(default)]
    pub always_skills: Vec<LockedSkill>,
}

/// One skill in a project's lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LockedSkill {
    /// Its name.
    pub name: String,
    /// The source it comes from.
    pub source: SkillSourceId,
    /// Its folder's path in the source.
    pub path: String,
    /// The source commit it was taken at.
    pub commit: Option<String>,
    /// Its folder's hash.
    pub tree: TreeHash,
    /// Whether it stays at this commit when the source moves on.
    pub pinned: bool,
}

/// The skills a project uses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillLock {
    /// Its skills, by name.
    pub skills: Vec<LockedSkill>,
    /// Whether the copies are committed to the repo rather than ignored.
    pub committed: bool,
    /// The library source its checkouts install as their devflow pack. A
    /// `maxd` from before it leaves this out, and a lock without one
    /// leaves it out of the mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<SkillPack>,
}

/// A library source a project uses whole, as its devflow pack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillPack {
    /// The source.
    pub source: SkillSourceId,
    /// The source commit it was taken at.
    pub commit: String,
    /// The source's whole tree at that commit.
    pub tree: TreeHash,
}

/// Where a checkout's devflow pack stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackState {
    /// Its installer is running.
    Installing {
        /// The tree it installs.
        tree: TreeHash,
    },
    /// Its installer ran.
    Installed {
        /// The tree it installed.
        tree: TreeHash,
    },
    /// Its installer failed, and runs again on the next apply.
    Failed {
        /// The tree it tried.
        tree: TreeHash,
        /// The last lines of its log.
        lines: Vec<String>,
    },
    /// Someone else put a `.devflow-vendor` in the checkout, so MaxHQ
    /// leaves it alone.
    Vendored,
}

/// Where one checkout of a project stands against its lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct CheckoutSkills {
    /// The checkout's absolute path on the device.
    pub path: String,
    /// Whether it has the lock's skills.
    pub applied: bool,
    /// The skills whose copies changed since they landed.
    pub drift: Vec<String>,
    /// Whether its running agents need a restart to see the skills.
    pub needs_restart: bool,
    /// Where its devflow pack stands, while the lock has one or someone
    /// else's is there. A `maxd` from before it leaves this out.
    #[serde(default)]
    pub pack: Option<PackState>,
    /// The links in the checkout, as paths in it, that lead out of it and
    /// so stopped skills landing through them. A `maxd` from before it
    /// leaves this out.
    #[serde(default)]
    pub outside: Vec<String>,
}

/// A project's skills on one device, as that device's `maxd` publishes
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProjectSkills {
    /// The project.
    pub project: ProjectId,
    /// The device that owns its lock.
    pub device: DeviceId,
    /// Its lock.
    pub lock: SkillLock,
    /// Each checkout on the device.
    pub checkouts: Vec<CheckoutSkills>,
}

/// Where a CLI finds a skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillPlace {
    /// In the project.
    Project,
    /// In your global folders.
    Global,
    /// In the set MaxHQ keeps global on every device.
    Always,
    /// From a plugin.
    Plugin,
    /// Shipped with the CLI.
    System,
}

/// What the scan noticed about a skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillMark {
    /// A global skill of the same name runs instead.
    Shadowed,
    /// The same name is in two places with the same tree.
    Same,
    /// The same name is in two places with different trees.
    Conflict,
    /// Another installer, such as devflow, owns its link.
    Vendored,
}

/// One skill a CLI sees from a place, as the scan finds it. The fields up
/// to `user_invocable` follow a provider's own skill shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct EffectiveSkill {
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: Option<String>,
    /// Its `SKILL.md` path on the device.
    pub path: String,
    /// The CLI's own word for where it is, when it has one.
    pub scope: Option<String>,
    /// Whether the CLI offers it.
    pub enabled: bool,
    /// The name to show, when it differs.
    pub display_name: Option<String>,
    /// A shorter description, when it has one.
    pub short_description: Option<String>,
    /// Only you can call it, so the composer offers it as a command.
    pub user_invocation_only: Option<bool>,
    /// Whether you can call it; when false only agents can.
    pub user_invocable: Option<bool>,
    /// Where the CLI finds it.
    pub place: SkillPlace,
    /// The CLI that sees it.
    pub cli: Vendor,
    /// What the scan noticed, if anything.
    pub mark: Option<SkillMark>,
}

/// A changed skill an agent wrote to its checkout's proposals folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SkillProposal {
    /// The proposal.
    pub id: SkillProposalId,
    /// The device it was found on.
    pub device: DeviceId,
    /// The project it was found in.
    pub project: ProjectId,
    /// The agent that wrote it, when known.
    pub agent: Option<AgentId>,
    /// The skill it changes.
    pub name: String,
    /// The tree it started from, none for a new skill.
    pub base: Option<TreeHash>,
    /// The changed folder's hash.
    pub tree: TreeHash,
    /// The checkout it's in, as an absolute path on the device.
    pub checkout: String,
    /// When it was found.
    pub created_at: Timestamp,
    /// What the changed skill does beyond its instructions.
    pub flags: SkillFlags,
    /// The source the skill comes from, none for a new skill.
    pub source: Option<SkillSourceId>,
}

/// What to do with a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    /// Commit it to its source.
    Approve,
    /// Throw it away.
    Reject,
}

/// What a proposal changes against its base tree, for the proposal
/// viewer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProposalDiff {
    /// The files that differ, as step 7's PR files tab shows them. A
    /// proposal with no base has every file added.
    pub files: Vec<PullFile>,
    /// What the changed skill does beyond its instructions.
    pub flags: SkillFlags,
    /// The tree the diff shows. A `maxd` from before it leaves this out.
    #[serde(default)]
    pub tree: Option<TreeHash>,
    /// Where approving it goes, as the device that keeps the library
    /// worked it out. A diff from any other device, or a `maxd` from
    /// before it, leaves this out.
    #[serde(default)]
    pub target: Option<ProposalTarget>,
}

/// Where approving a proposal goes, worked out from the library and the
/// proposal's tree, whatever the proposal says of itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProposalTarget {
    /// The skill it writes.
    pub name: String,
    /// The source it goes to, or none for your personal source.
    pub source: Option<SkillSourceId>,
    /// The skill's tree in that source now, or none when it adds a skill.
    pub base: Option<TreeHash>,
}

/// What the owner read when they approved a proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProposalSeen {
    /// The tree the diff showed.
    pub tree: TreeHash,
    /// The tree the diff compared it with, or none when it added a skill.
    pub base: Option<TreeHash>,
}

/// What tidying a device's global skills would do, for you to approve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GlobalsPlan {
    /// Names this plan, so applying it applies what you saw.
    pub hash: String,
    /// The folders that move into the library.
    pub moves: Vec<String>,
    /// The links that appear.
    pub links: Vec<String>,
    /// The copies that go to the backup folder.
    pub backups: Vec<String>,
    /// The skills a repo you installed them from tracks. Each repo joins
    /// the library as a third-party source, and the copies go to the
    /// backup. A `maxd` from before it leaves this out.
    #[serde(default)]
    pub joins: Vec<GlobalsJoin>,
    /// Same-named skills whose copies differ. They stay where they are
    /// until you pick the copy that goes into the library. A `maxd` from
    /// before it leaves this out.
    #[serde(default)]
    pub conflicts: Vec<GlobalsConflict>,
    /// Whether the device has a tidy to undo. A `maxd` from before it
    /// leaves this out, and reads as false.
    #[serde(default)]
    pub undo: bool,
}

/// A repo the tidy adds as a source, with the copies it takes from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GlobalsJoin {
    /// What the source is called.
    pub name: String,
    /// The repo.
    pub url: String,
    /// The copies that go to the backup once it joins.
    pub paths: Vec<String>,
}

/// One skill's copies that differ, for you to pick from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GlobalsConflict {
    /// The skill.
    pub name: String,
    /// Each copy, the one the CLIs prefer first.
    pub paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tree_hash_is_a_bare_string() {
        let tree = TreeHash("ab12".into());
        assert_eq!(serde_json::to_value(&tree).unwrap(), json!("ab12"));
    }

    #[test]
    fn a_source_round_trips() {
        let id = SkillSourceId::derive("example/agent-skills");
        let source = SkillSource {
            id,
            name: "agent-skills".into(),
            kind: SourceKind::Git {
                url: "https://github.com/owner/repo".into(),
            },
            default_ref: None,
            trust: SourceTrust::ThirdParty,
            commit: Some("1234abc".into()),
            fetched_at: None,
            problem: None,
            skills: vec![SkillInfo {
                name: "unslop".into(),
                description: "Cut AI tells.".into(),
                tree: TreeHash("ab12".into()),
                path: "skills/unslop".into(),
                source: id,
                flags: SkillFlags {
                    scripts: true,
                    ..SkillFlags::default()
                },
                listing_chars: 120,
            }],
            pack: None,
            unpushed: false,
        };
        let value = serde_json::to_value(&source).unwrap();
        assert_eq!(value["kind"]["kind"], "git");
        assert_eq!(value["trust"], "third_party");
        assert_eq!(
            serde_json::from_value::<SkillSource>(value).unwrap(),
            source
        );
    }

    #[test]
    fn a_plan_from_an_older_maxd_has_no_groups_and_no_undo() {
        let plan: GlobalsPlan = serde_json::from_value(json!({
            "hash": "ab12", "moves": [], "links": [], "backups": []
        }))
        .unwrap();
        assert!(plan.joins.is_empty() && plan.conflicts.is_empty());
        assert!(!plan.undo);
    }

    #[test]
    fn a_lock_from_an_older_maxd_has_no_pack() {
        let lock: SkillLock =
            serde_json::from_value(json!({ "skills": [], "committed": false })).unwrap();
        assert_eq!(lock.pack, None);
        let checkout: CheckoutSkills = serde_json::from_value(json!({
            "path": "/work/repo", "applied": true, "drift": [], "needs_restart": false
        }))
        .unwrap();
        assert_eq!(checkout.pack, None);
        assert!(checkout.outside.is_empty());
    }

    #[test]
    fn a_pack_state_says_which_it_is() {
        let failed = PackState::Failed {
            tree: TreeHash("ab12".into()),
            lines: vec!["no uv".into()],
        };
        let value = serde_json::to_value(&failed).unwrap();
        assert_eq!(
            value,
            json!({ "state": "failed", "tree": "ab12", "lines": ["no uv"] })
        );
        assert_eq!(serde_json::from_value::<PackState>(value).unwrap(), failed);
        assert_eq!(
            serde_json::to_value(PackState::Vendored).unwrap(),
            json!({ "state": "vendored" })
        );
    }
}
