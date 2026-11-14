// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The GitHub repository a workspace checks out, and where it lives in the
//! container.

use std::fmt;

use maxhq_core::names;
use maxhq_protocol::ids::{DeviceId, ProjectId};

use crate::recipe::WORK;

/// A repository on github.com.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    owner: String,
    name: String,
}

/// The URL forms a github.com remote takes, up to the owner.
const PREFIXES: &[&str] = &[
    "https://github.com/",
    "http://github.com/",
    "ssh://git@github.com/",
    "git://github.com/",
    "git@github.com:",
    "github.com/",
];

impl Repo {
    /// Reads a clone URL or `owner/name`. None for a remote that isn't on
    /// github.com.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let rest = PREFIXES
            .iter()
            .find_map(|prefix| text.strip_prefix(prefix))
            .or_else(|| (!text.contains(':')).then_some(text))?;
        let rest = rest.trim_end_matches('/');
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let (owner, name) = rest.split_once('/')?;
        let allowed = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        };
        (allowed(owner) && allowed(name)).then(|| Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The name after the owner, as GitHub shows it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `owner/name`, which the GitHub App mints tokens for.
    pub fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The URL the container clones.
    pub fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }

    /// The checkout's folder in the container.
    pub fn dir(&self) -> String {
        format!("{WORK}/{}", self.name)
    }

    /// The project every checkout of this repository belongs to, as the
    /// observers name it.
    pub fn project(&self, device: DeviceId) -> ProjectId {
        names::project(device, "", Some(&self.clone_url())).id
    }

    /// The project's name, as the observers give it.
    pub fn title(&self, device: DeviceId) -> String {
        names::project(device, "", Some(&self.clone_url())).name
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_github_remote_form() {
        for text in [
            "https://github.com/Example-User/MaxHQ.git",
            "https://github.com/Example-User/MaxHQ",
            "https://github.com/Example-User/MaxHQ/",
            "git@github.com:Example-User/MaxHQ.git",
            "ssh://git@github.com/Example-User/MaxHQ.git",
            "github.com/Example-User/MaxHQ",
            "Example-User/MaxHQ",
        ] {
            let repo = Repo::parse(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(repo.path(), "Example-User/MaxHQ", "{text}");
        }
        let repo = Repo::parse("Example-User/MaxHQ").unwrap();
        assert_eq!(
            repo.clone_url(),
            "https://github.com/Example-User/MaxHQ.git"
        );
        assert_eq!(repo.dir(), "/home/agent/work/MaxHQ");
        assert_eq!(repo.name(), "MaxHQ");
    }

    #[test]
    fn refuses_other_hosts_and_odd_paths() {
        for text in [
            "https://gitlab.com/a/b.git",
            "git@gitlab.com:a/b.git",
            "a",
            "a/b/c",
            "a/",
            "../b",
            "a/b c",
            "",
        ] {
            assert_eq!(Repo::parse(text), None, "{text}");
        }
    }

    #[test]
    fn names_the_project_as_the_observers_do() {
        let device = DeviceId::derive("devbox");
        let repo = Repo::parse("Example-User/MaxHQ").unwrap();
        let observed = names::project(
            DeviceId::derive("mac"),
            "/Users/max/MaxHQ",
            Some("git@github.com:example-user/maxhq.git"),
        );
        assert_eq!(repo.project(device), observed.id);
        assert_eq!(repo.title(device), "maxhq");
    }
}
