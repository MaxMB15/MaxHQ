// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where a tool's latest release and its checksums come from, when its
//! definition doesn't pin them: a GitHub release, Node's own index, npm's
//! registry or PyPI. A file is fetched only with a SHA-256 its publisher
//! gave.

use futures::future::BoxFuture;
use serde::Deserialize;

use super::{fetch, ArchiveFile, InstallError};

/// Where a tool's releases are published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feed {
    /// A GitHub repo's releases.
    GitHub {
        /// The repo, as `owner/name`.
        repo: &'static str,
        /// What its tags put before the version, such as `v`.
        prefix: &'static str,
        /// The assets a target needs: the first is the tool, the rest are
        /// taken when the release has them. Each may hold `{version}` and
        /// `{target}`, or `{gh}` for the GitHub CLI's own platform and archive
        /// type.
        assets: &'static [&'static str],
    },
    /// Node.js's own downloads.
    Node,
}

/// A release, ready to fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: String,
    /// What a target needs from it, with checksums.
    pub files: Vec<ArchiveFile>,
}

/// Looks releases up.
pub trait Releases: Send + Sync {
    /// The release of `feed` at `version`, or the latest, for `target`.
    fn archive<'a>(
        &'a self,
        feed: &'a Feed,
        version: Option<&'a str>,
        target: &'a str,
    ) -> BoxFuture<'a, Result<Release, InstallError>>;

    /// An npm package's latest version.
    fn npm_latest<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Result<String, InstallError>>;

    /// A Python package's latest version.
    fn pypi_latest<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Result<String, InstallError>>;
}

/// The publishers themselves, at addresses a test may change.
#[derive(Clone, Debug)]
pub struct Publishers {
    /// GitHub's API.
    pub github: String,
    /// Node.js's downloads.
    pub node: String,
    /// npm's registry.
    pub npm: String,
    /// PyPI.
    pub pypi: String,
}

impl Default for Publishers {
    fn default() -> Self {
        Self {
            github: "https://api.github.com".to_owned(),
            node: "https://nodejs.org/dist".to_owned(),
            npm: "https://registry.npmjs.org".to_owned(),
            pypi: "https://pypi.org/pypi".to_owned(),
        }
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct NodeRelease {
    version: String,
    lts: serde_json::Value,
}

/// Reads JSON from `url`, failing in words.
async fn json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, InstallError> {
    let text = fetch::text(url).await.map_err(InstallError::Failed)?;
    serde_json::from_str(&text)
        .map_err(|_| InstallError::Failed(format!("couldn't read the answer from {url}")))
}

/// The SHA-256 for `name` in a `sha256sum` list.
pub fn sum_for(list: &str, name: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && sum.len() == 64).then(|| sum.to_ascii_lowercase())
    })
}

/// The GitHub CLI's name for a target's archive, after its version.
fn gh_platform(target: &str) -> Option<&'static str> {
    Some(match target {
        "aarch64-apple-darwin" => "macOS_arm64.zip",
        "x86_64-apple-darwin" => "macOS_amd64.zip",
        "x86_64-unknown-linux-musl" => "linux_amd64.tar.gz",
        "aarch64-unknown-linux-musl" => "linux_arm64.tar.gz",
        _ => return None,
    })
}

/// Node.js's name for a target's platform.
fn node_platform(target: &str) -> Option<&'static str> {
    Some(match target {
        "aarch64-apple-darwin" => "darwin-arm64",
        "x86_64-apple-darwin" => "darwin-x64",
        "x86_64-unknown-linux-musl" => "linux-x64",
        "aarch64-unknown-linux-musl" => "linux-arm64",
        _ => return None,
    })
}

impl Publishers {
    async fn github(
        &self,
        repo: &str,
        prefix: &str,
        assets: &[&str],
        version: Option<&str>,
        target: &str,
    ) -> Result<Release, InstallError> {
        let url = match version {
            Some(version) => format!(
                "{}/repos/{repo}/releases/tags/{prefix}{version}",
                self.github
            ),
            None => format!("{}/repos/{repo}/releases/latest", self.github),
        };
        let release: GitHubRelease = json(&url).await?;
        let version = release
            .tag_name
            .strip_prefix(prefix)
            .unwrap_or(&release.tag_name)
            .to_owned();
        let gh = gh_platform(target);
        let mut files = Vec::new();
        let mut sums: Option<String> = None;
        for (at, pattern) in assets.iter().enumerate() {
            if pattern.contains("{gh}") && gh.is_none() {
                return Err(InstallError::NoTarget(target.to_owned()));
            }
            let name = pattern
                .replace("{version}", &version)
                .replace("{target}", target)
                .replace("{gh}", gh.unwrap_or(""));
            let Some(asset) = release.assets.iter().find(|asset| asset.name == name) else {
                if at == 0 {
                    return Err(InstallError::NoTarget(target.to_owned()));
                }
                continue;
            };
            let mut sha256 = asset
                .digest
                .as_deref()
                .and_then(|digest| digest.strip_prefix("sha256:"))
                .map(str::to_owned);
            if sha256.is_none() {
                sha256 = self.github_sum(&release, &name, &mut sums).await?;
            }
            files.push(ArchiveFile {
                target: target.to_owned(),
                url: asset.browser_download_url.clone(),
                sha256,
                size: asset.size,
            });
        }
        Ok(Release { version, files })
    }

    /// A SHA-256 from the release's own checksum files, when it has them.
    async fn github_sum(
        &self,
        release: &GitHubRelease,
        name: &str,
        sums: &mut Option<String>,
    ) -> Result<Option<String>, InstallError> {
        let own = format!("{name}.sha256");
        if let Some(asset) = release.assets.iter().find(|asset| asset.name == own) {
            let text = fetch::text(&asset.browser_download_url)
                .await
                .map_err(InstallError::Failed)?;
            let sum = text.split_whitespace().next().unwrap_or("");
            return Ok((sum.len() == 64).then(|| sum.to_ascii_lowercase()));
        }
        if sums.is_none() {
            let Some(asset) = release
                .assets
                .iter()
                .find(|asset| asset.name.ends_with("checksums.txt"))
            else {
                return Ok(None);
            };
            *sums = Some(
                fetch::text(&asset.browser_download_url)
                    .await
                    .map_err(InstallError::Failed)?,
            );
        }
        Ok(sums.as_deref().and_then(|list| sum_for(list, name)))
    }

    async fn node(&self, version: Option<&str>, target: &str) -> Result<Release, InstallError> {
        let Some(platform) = node_platform(target) else {
            return Err(InstallError::NoTarget(target.to_owned()));
        };
        let version = match version {
            Some(version) => version.trim_start_matches('v').to_owned(),
            None => {
                let index: Vec<NodeRelease> = json(&format!("{}/index.json", self.node)).await?;
                let latest = index
                    .into_iter()
                    .find(|release| release.lts.is_string())
                    .ok_or_else(|| {
                        InstallError::Failed("Node.js lists no long-term release.".to_owned())
                    })?;
                latest.version.trim_start_matches('v').to_owned()
            }
        };
        let name = format!("node-v{version}-{platform}.tar.gz");
        let list = fetch::text(&format!("{}/v{version}/SHASUMS256.txt", self.node))
            .await
            .map_err(InstallError::Failed)?;
        let sha256 = sum_for(&list, &name);
        Ok(Release {
            files: vec![ArchiveFile {
                target: target.to_owned(),
                url: format!("{}/v{version}/{name}", self.node),
                sha256,
                size: None,
            }],
            version,
        })
    }
}

impl Releases for Publishers {
    fn archive<'a>(
        &'a self,
        feed: &'a Feed,
        version: Option<&'a str>,
        target: &'a str,
    ) -> BoxFuture<'a, Result<Release, InstallError>> {
        Box::pin(async move {
            match feed {
                Feed::GitHub {
                    repo,
                    prefix,
                    assets,
                } => self.github(repo, prefix, assets, version, target).await,
                Feed::Node => self.node(version, target).await,
            }
        })
    }

    fn npm_latest<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Result<String, InstallError>> {
        Box::pin(async move {
            #[derive(Deserialize)]
            struct Latest {
                version: String,
            }
            let latest: Latest = json(&format!("{}/{package}/latest", self.npm)).await?;
            Ok(latest.version)
        })
    }

    fn pypi_latest<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Result<String, InstallError>> {
        Box::pin(async move {
            #[derive(Deserialize)]
            struct Info {
                version: String,
            }
            #[derive(Deserialize)]
            struct Project {
                info: Info,
            }
            let project: Project = json(&format!("{}/{package}/json", self.pypi)).await?;
            Ok(project.info.version)
        })
    }
}
