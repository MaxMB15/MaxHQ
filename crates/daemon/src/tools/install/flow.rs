// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: a version
// installs into a staging folder, its download is checked against the
// release's SHA-256 before anything unpacks, an install that is already
// whole is reused, and a finished one is published by rename. The npm
// source is the same code: a package installs with `npm install --prefix`
// into a folder of its own. Its command comes from its manifest, picked the
// way that code does.

//! An install and a removal, step by step, in one place. Each reports its
//! phase as it goes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use maxhq_protocol::state::{HelperTool, ToolKind, ToolPhase, ToolState};

use super::releases::Releases;
use super::scripts::{CHECK, FINISH, PUBLISH, REMOVE, RUN, STAGE, UNPACK};
use super::{
    lock, name, ArchiveFile, Error, InstallDef, InstallError, Installed, Report, Source, Spot,
};
use crate::tools::defs;

/// How long an install waits for another install of the same tool.
const WAIT: Duration = Duration::from_secs(30 * 60);
/// The most a tool's own install script may be.
const SCRIPT_MOST: u64 = 1024 * 1024;
/// How often a download reports its bytes.
const REPORT_EVERY: u64 = 1024 * 1024;
/// Where npm and uv wrappers go inside a version.
const WRAPPERS: &str = ".maxhq-bin";

/// Runs npm with MaxHQ's Node first on `PATH`. It takes Node's `bin` and
/// what npm takes.
const NPM: &str = r#"d="$1"; shift
PATH="$d:$PATH"; export PATH
exec "$d/npm" "$@"
"#;

/// Writes a wrapper from stdin. It takes the wrapper's path.
const WRAPPER: &str = r#"set -eu
mkdir -p "${1%/*}"
cat > "$1"
chmod 755 "$1"
"#;

/// Exits 0 when something holds the path, even a broken link.
const THERE: &str = r#"[ -e "$1" ] || [ -L "$1" ]"#;

fn running(phase: ToolPhase, message: String) -> ToolState {
    ToolState::Running {
        message,
        phase: Some(phase),
        bytes_done: None,
        bytes_total: None,
    }
}

/// Installs `def` in `spot` at `version`, or the latest.
pub async fn install(
    spot: &Spot<'_>,
    def: &InstallDef,
    version: Option<&str>,
    releases: &dyn Releases,
    report: Report<'_>,
) -> Result<Installed, Error> {
    install_as(spot, def, &name(&def.kind), version, releases, report).await
}

/// Installs `def` in `spot` under the folder `name`, for a tool the views
/// don't list as one, such as the llama-server local models run on. Its
/// archives must be pinned, since the kind names no release feed.
pub async fn install_as(
    spot: &Spot<'_>,
    def: &InstallDef,
    name: &str,
    version: Option<&str>,
    releases: &dyn Releases,
    report: Report<'_>,
) -> Result<Installed, Error> {
    let dir = format!("{}/{name}", spot.layout.tools());
    let _turn = lock::turn(&spot.layout.label, &dir).await;
    report(running(
        ToolPhase::Resolving,
        format!("Getting ready to install {name}"),
    ));
    let held = lock::take(spot, &dir, name, WAIT).await?;
    let job = Job {
        spot,
        def,
        name,
        dir: &dir,
        report,
    };
    let result = match &def.source {
        Source::Script { url } => job.script(url, version).await,
        Source::Archive {
            files,
            confirmed_unchecked,
        } => {
            job.archive(files, *confirmed_unchecked, version, releases)
                .await
        }
        Source::Npm { package } => job.npm(package, version, releases).await,
        Source::Uv { package } => job.uv(package, version, releases).await,
    };
    held.release(spot).await;
    result
}

/// Removes a tool MaxHQ installed in `spot`, with the links it made.
pub async fn remove(spot: &Spot<'_>, kind: &ToolKind) -> Result<(), Error> {
    let name = name(kind);
    let dir = spot.layout.dir(kind);
    let _turn = lock::turn(&spot.layout.label, &dir).await;
    let held = lock::take(spot, &dir, &name, WAIT).await?;
    let output = spot.sh(REMOVE, &[&dir]).await;
    held.release(spot).await;
    let output = output?;
    match output.status {
        0 => Ok(()),
        4 => Err(Error::Install(InstallError::Failed(format!(
            "MaxHQ didn't install {name} here."
        )))),
        _ => Err(spot.failed(REMOVE, &output)),
    }
}

/// One install under way.
struct Job<'a> {
    spot: &'a Spot<'a>,
    def: &'a InstallDef,
    name: &'a str,
    dir: &'a str,
    report: Report<'a>,
}

/// What a version's record names.
#[derive(Default)]
struct Record {
    entries: Vec<String>,
    links: Vec<(String, String)>,
}

impl Record {
    fn read(text: &str) -> Self {
        let mut record = Record::default();
        for line in text.lines() {
            if let Some(entry) = line.strip_prefix("entry ") {
                record.entries.push(entry.to_owned());
            } else if let Some((link, entry)) = line
                .strip_prefix("link ")
                .and_then(|rest| rest.split_once(' '))
            {
                record.links.push((link.to_owned(), entry.to_owned()));
            }
        }
        record
    }
}

impl Job<'_> {
    /// A version a publisher named, if it's safe as a folder name, a package
    /// spec and wrapper text.
    fn upstream(&self, version: String) -> Result<String, Error> {
        if crate::acp_registry::bounded_version(&version) {
            return Ok(version);
        }
        Err(Error::Install(InstallError::Failed(format!(
            "{} was published with a version MaxHQ can't use.",
            self.name
        ))))
    }

    fn say(&self, phase: ToolPhase, what: &str) {
        (self.report)(running(phase, format!("{what} {}", self.name)));
    }

    /// The command the tool's link takes.
    fn binary(&self, package: Option<&str>) -> String {
        match (&self.def.binary, package) {
            (Some(binary), _) => binary.clone(),
            (None, Some(package)) => package.rsplit('/').next().unwrap_or(package).to_owned(),
            (None, None) => self.name.to_owned(),
        }
    }

    /// The record of a version that's whole, read and as text, or `None`.
    async fn whole(&self, version_dir: &str) -> Result<Option<(Record, String)>, Error> {
        if !self.spot.sh(CHECK, &[version_dir]).await?.success() {
            return Ok(None);
        }
        let text = self
            .spot
            .exec
            .read(&format!("{version_dir}/.install-complete"))
            .await?
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&text).into_owned();
        Ok(Some((Record::read(&text), text)))
    }

    /// A fresh staging folder, after clearing what an earlier install left.
    async fn stage(&self) -> Result<String, Error> {
        let staging = format!(".staging-{}", uuid::Uuid::now_v7().simple());
        self.spot.sh_ok(STAGE, &[self.dir, &staging]).await?;
        Ok(staging)
    }

    /// Runs `work` in a staging folder, and clears the folder if it fails.
    async fn staged<T>(
        &self,
        staging: &str,
        work: impl std::future::Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        let result = work.await;
        if result.is_err() {
            let path = format!("{}/{staging}", self.dir);
            let _ = self.spot.run(&["rm", "-rf", &path], None).await;
        }
        result
    }

    /// Publishes a version: its record, `current` and links. Without a
    /// staging folder the version is already in place.
    async fn publish(
        &self,
        staging: Option<&str>,
        version: &str,
        record: &str,
        links: &[(String, String)],
    ) -> Result<Installed, Error> {
        self.say(ToolPhase::Linking, "Linking");
        let bin = self.spot.layout.bin();
        let replace = if self.spot.layout.replace_links {
            "1"
        } else {
            "0"
        };
        let mut args = vec![
            self.dir,
            staging.unwrap_or(".staging-none"),
            version,
            &bin,
            replace,
            record,
        ];
        for (link, entry) in links {
            args.push(link);
            args.push(entry);
        }
        let output = self.spot.sh_ok(PUBLISH, &args).await?;
        let linked: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("linked "))
            .map(str::to_owned)
            .collect();
        let path = match links.first() {
            Some((link, _)) if linked.contains(&format!("{bin}/{link}")) => format!("{bin}/{link}"),
            Some((_, entry)) => format!("{}/current/{entry}", self.dir),
            None => {
                let record = Record::read(record);
                let entry = record.entries.first().map(String::as_str).unwrap_or("");
                format!("{}/current/{entry}", self.dir)
            }
        };
        Ok(Installed {
            version: version.to_owned(),
            path,
            root: format!("{}/{version}", self.dir),
        })
    }

    async fn archive(
        &self,
        files: &[ArchiveFile],
        confirmed_unchecked: bool,
        version: Option<&str>,
        releases: &dyn Releases,
    ) -> Result<Installed, Error> {
        let target = &self.spot.layout.target;
        let (version, files) = if files.is_empty() {
            let Some(feed) = defs::feed(&self.def.kind) else {
                return Err(Error::Install(InstallError::Failed(format!(
                    "MaxHQ doesn't know where {} is published.",
                    self.name
                ))));
            };
            let release = releases.archive(&feed, version, target).await?;
            (self.upstream(release.version)?, release.files)
        } else {
            let Some(version) = version else {
                return Err(Error::Install(InstallError::Failed(format!(
                    "{} needs a version to install.",
                    self.name
                ))));
            };
            let files: Vec<ArchiveFile> = files
                .iter()
                .filter(|file| file.target == *target)
                .cloned()
                .collect();
            (version.to_owned(), files)
        };
        if files.is_empty() {
            return Err(Error::Install(InstallError::NoTarget(target.clone())));
        }
        if !confirmed_unchecked && files.iter().any(|file| file.sha256.is_none()) {
            return Err(Error::Install(InstallError::Unchecked));
        }
        let version_dir = format!("{}/{version}", self.dir);
        if let Some((record, text)) = self.whole(&version_dir).await? {
            return self.publish(None, &version, &text, &record.links).await;
        }
        let staging = self.stage().await?;
        let (record, links) = self
            .staged(&staging, self.fetch_archives(&staging, &version, &files))
            .await?;
        self.publish(Some(&staging), &version, &record, &links)
            .await
    }

    /// Downloads, checks and unpacks a version's files, and gives its
    /// record and links.
    async fn fetch_archives(
        &self,
        staging: &str,
        version: &str,
        files: &[ArchiveFile],
    ) -> Result<(String, Vec<(String, String)>), Error> {
        let spot = self.spot;
        let staging_dir = format!("{}/{staging}", self.dir);
        let total: Option<u64> = files.iter().map(|file| file.size).sum();
        let mut before = 0u64;
        let mut names = Vec::new();
        let mut record = format!("version {version}\ntarget {}\n", spot.layout.target);
        for file in files {
            let file_name = download_name(&file.url);
            let path = format!("{staging_dir}/dl/{file_name}");
            self.say(ToolPhase::Downloading, "Downloading");
            let last = AtomicU64::new(0);
            let progress = |done: u64, size: Option<u64>| {
                let at = before + done;
                let seen = last.load(Ordering::Relaxed);
                if at < seen + REPORT_EVERY && Some(done) != size {
                    return;
                }
                last.store(at, Ordering::Relaxed);
                (self.report)(ToolState::Running {
                    message: format!("Downloading {}", self.name),
                    phase: Some(ToolPhase::Downloading),
                    bytes_done: Some(at),
                    bytes_total: total.or(size.map(|size| before + size)),
                });
            };
            let got = spot
                .exec
                .download(&file.url, &path, spot.layout.user, file.size, &progress)
                .await?;
            self.say(ToolPhase::Verifying, "Checking");
            if let Some(size) = file.size
                && size != got.size
            {
                return Err(Error::Install(InstallError::Failed(format!(
                    "The download of {} was {} bytes, not {size}.",
                    self.name, got.size
                ))));
            }
            if let Some(want) = &file.sha256
                && !want.eq_ignore_ascii_case(&got.sha256)
            {
                return Err(Error::Install(InstallError::Checksum {
                    want: want.to_ascii_lowercase(),
                    got: got.sha256,
                }));
            }
            record.push_str(&format!("sha256 {file_name} {}\n", got.sha256));
            before += got.size;
            names.push(file_name);
        }
        self.say(ToolPhase::Installing, "Installing");
        let mut args = vec![staging_dir.as_str(), spot.layout.target.as_str()];
        args.extend(names.iter().map(String::as_str));
        let output = spot.sh_ok(UNPACK, &args).await?;
        let programs: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("./"))
            .map(str::to_owned)
            .collect();
        let binary = self.binary(None);
        let entry = programs
            .iter()
            .find(|program| **program == binary)
            .or_else(|| {
                programs
                    .iter()
                    .find(|program| **program == format!("bin/{binary}"))
            })
            .or(programs.first())
            .cloned()
            .ok_or_else(|| {
                Error::Install(InstallError::Failed(format!(
                    "The download of {} has no program in it.",
                    self.name
                )))
            })?;
        let mut links = Vec::new();
        if let Some(binary) = &self.def.binary {
            links.push((binary.clone(), entry.clone()));
            // Programs that come with it, such as Codex's code-mode host
            // or uv's uvx, get links too.
            for program in &programs {
                if !program.contains('/')
                    && *program != entry
                    && program.starts_with(binary.as_str())
                {
                    links.push((program.clone(), program.clone()));
                }
            }
        }
        record.push_str(&format!("entry {entry}\n"));
        for (link, program) in &links {
            if *program != entry {
                record.push_str(&format!("entry {program}\n"));
            }
            record.push_str(&format!("link {link} {program}\n"));
        }
        Ok((record, links))
    }

    async fn script(&self, url: &str, version: Option<&str>) -> Result<Installed, Error> {
        let spot = self.spot;
        let binary = self.binary(None);
        let link = format!("{}/{binary}", spot.layout.bin());
        let ours = spot.exec.read(&format!("{}/script", self.dir)).await?;
        if let Some(ours) = &ours {
            let recorded = String::from_utf8_lossy(ours).trim().to_owned();
            if version.is_none_or(|version| version == recorded)
                && let Some(installed) = self.ran(&link).await?
            {
                return Ok(installed);
            }
        } else if !spot.layout.replace_links && spot.sh(THERE, &[&link]).await?.success() {
            return Err(Error::Install(InstallError::Failed(format!(
                "{} is already installed here, and MaxHQ leaves yours alone.",
                self.name
            ))));
        }
        let staging = self.stage().await?;
        let installed = self
            .staged(&staging, async {
                let path = format!("{}/{staging}/dl/install.sh", self.dir);
                self.say(ToolPhase::Downloading, "Downloading");
                let quiet = |_: u64, _: Option<u64>| {};
                spot.exec
                    .download(url, &path, spot.layout.user, Some(SCRIPT_MOST), &quiet)
                    .await?;
                self.say(ToolPhase::Installing, "Installing");
                let mut args = vec![path.as_str()];
                args.extend(version);
                spot.sh_ok(RUN, &args).await?;
                self.ran(&link).await?.ok_or_else(|| {
                    Error::Install(InstallError::Failed(format!(
                        "{}'s installer finished, but {link} doesn't run.",
                        self.name
                    )))
                })
            })
            .await?;
        self.say(ToolPhase::Linking, "Linking");
        spot.sh_ok(FINISH, &[self.dir, &installed.version, &link])
            .await?;
        Ok(installed)
    }

    /// What the link runs as, when it runs.
    async fn ran(&self, link: &str) -> Result<Option<Installed>, Error> {
        let output = self.spot.command(&[link, "--version"]).await?;
        if !output.success() {
            return Ok(None);
        }
        let version = defs::version_text(&String::from_utf8_lossy(&output.stdout))
            .unwrap_or_else(|| "unknown".to_owned());
        Ok(Some(Installed {
            version,
            path: link.to_owned(),
            root: self.dir.to_owned(),
        }))
    }

    /// Makes sure a helper is current, and gives its `current` folder.
    async fn helper(&self, tool: HelperTool, releases: &dyn Releases) -> Result<String, Error> {
        let kind = ToolKind::Helper { tool };
        let current = format!("{}/current", self.spot.layout.dir(&kind));
        if self.whole(&current).await?.is_none() {
            let Some(def) = defs::plan(&kind, false) else {
                return Err(Error::Install(InstallError::NotBuilt));
            };
            Box::pin(install(self.spot, &def, None, releases, self.report)).await?;
        }
        Ok(current)
    }

    /// Writes a wrapper into a staging folder's `files`.
    async fn wrapper(&self, staging: &str, binary: &str, text: &str) -> Result<String, Error> {
        let entry = format!("{WRAPPERS}/{binary}");
        let path = format!("{}/{staging}/files/{entry}", self.dir);
        let cmd = ["sh", "-c", WRAPPER, "sh", path.as_str()];
        let output = self.spot.run(&cmd, Some(text.as_bytes())).await?;
        if !output.success() {
            return Err(self.spot.failed(WRAPPER, &output));
        }
        Ok(entry)
    }

    async fn npm(
        &self,
        package: &str,
        version: Option<&str>,
        releases: &dyn Releases,
    ) -> Result<Installed, Error> {
        let node = self.helper(HelperTool::Node, releases).await?;
        let version = match version {
            Some(version) => version.to_owned(),
            None => self.upstream(releases.npm_latest(package).await?)?,
        };
        let version_dir = format!("{}/{version}", self.dir);
        if let Some((record, text)) = self.whole(&version_dir).await? {
            return self.publish(None, &version, &text, &record.links).await;
        }
        let staging = self.stage().await?;
        let (record, links) = self
            .staged(&staging, async {
                self.say(ToolPhase::Installing, "Installing");
                let prefix = format!("{}/{staging}/files", self.dir);
                let node_bin = format!("{node}/bin");
                let spec = format!("{package}@{version}");
                let args = [
                    node_bin.as_str(),
                    "install",
                    "--prefix",
                    &prefix,
                    "--no-save",
                    "--no-audit",
                    "--no-fund",
                    "--no-update-notifier",
                    "--omit=dev",
                    &spec,
                ];
                self.spot.sh_ok(NPM, &args).await?;
                let manifest = self
                    .spot
                    .exec
                    .read(&format!("{prefix}/node_modules/{package}/package.json"))
                    .await?;
                let binary = manifest
                    .and_then(|text| npm_command(&text, package, self.def.binary.as_deref()))
                    .unwrap_or_else(|| self.binary(Some(package)));
                let program = format!("{}/current/node_modules/.bin/{binary}", self.dir);
                let text =
                    format!("#!/bin/sh\nPATH=\"{node}/bin:$PATH\" exec \"{program}\" \"$@\"\n");
                let entry = self.wrapper(&staging, &binary, &text).await?;
                let entries = vec![format!("node_modules/.bin/{binary}"), entry.clone()];
                let links = vec![(binary.clone(), entry)];
                Ok((record_text(package, &version, &entries, &links), links))
            })
            .await?;
        self.publish(Some(&staging), &version, &record, &links)
            .await
    }

    async fn uv(
        &self,
        package: &str,
        version: Option<&str>,
        releases: &dyn Releases,
    ) -> Result<Installed, Error> {
        let uv = self.helper(HelperTool::Uv, releases).await?;
        let version = match version {
            Some(version) => version.to_owned(),
            None => self.upstream(releases.pypi_latest(package).await?)?,
        };
        let binary = self.binary(Some(package));
        let staging = self.stage().await?;
        let (record, links) = self
            .staged(&staging, async {
                self.say(ToolPhase::Installing, "Installing");
                // uvx fetches the package the first time it runs.
                let text = format!(
                    "#!/bin/sh\nexec \"{uv}/uvx\" --from \"{package}=={version}\" \"{binary}\" \"$@\"\n"
                );
                let entry = self.wrapper(&staging, &binary, &text).await?;
                let links = vec![(binary.clone(), entry.clone())];
                Ok((
                    record_text(package, &version, &[entry], &links),
                    links,
                ))
            })
            .await?;
        self.publish(Some(&staging), &version, &record, &links)
            .await
    }
}

/// The command an npm package's manifest names: the package's own name for
/// a lone `bin`, else `wanted`, the only one, the one named as the package,
/// or the first when they all run the same file.
pub(super) fn npm_command(manifest: &[u8], package: &str, wanted: Option<&str>) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    let own = package.rsplit('/').next().unwrap_or(package);
    let bin = match manifest.get("bin")? {
        serde_json::Value::String(_) => return Some(own.to_owned()),
        serde_json::Value::Object(bin) => bin,
        _ => return None,
    };
    let fits = |name: &&String| {
        !name.is_empty()
            && name.len() <= 128
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && !name.starts_with('.')
    };
    let mut names: Vec<&String> = bin.keys().filter(fits).collect();
    names.sort();
    let pick = |name: &str| {
        names
            .iter()
            .find(|have| **have == name)
            .map(|name| (*name).clone())
    };
    if let Some(name) = wanted.and_then(pick) {
        return Some(name);
    }
    if names.len() == 1 {
        return Some(names[0].clone());
    }
    if let Some(name) = pick(own) {
        return Some(name);
    }
    let mut paths = names.iter().filter_map(|name| bin.get(*name));
    let first = paths.next()?;
    paths.all(|path| path == first).then(|| names[0].clone())
}

/// The record for a package version.
fn record_text(
    package: &str,
    version: &str,
    entries: &[String],
    links: &[(String, String)],
) -> String {
    let mut record = format!("version {version}\npackage {package}\n");
    for entry in entries {
        record.push_str(&format!("entry {entry}\n"));
    }
    for (link, entry) in links {
        record.push_str(&format!("link {link} {entry}\n"));
    }
    record
}

/// The name a download is saved as: the end of its URL, with anything a
/// shell would read otherwise swapped out.
fn download_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name: String = path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_start_matches('.');
    if name.is_empty() {
        "download".to_owned()
    } else {
        name.to_owned()
    }
}
