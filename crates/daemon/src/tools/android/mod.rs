// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Android SDK on a Mac, for the emulator step 8's leases boot.
//!
//! Google's command-line tools go through the installer into maxd's tools
//! folder, with a JDK from it too when the Mac has no Java that's new
//! enough. The SDK itself lives beside them in `tools/android`, where
//! `sdkmanager` installs `platform-tools`, `emulator` and one system image
//! for the Mac's CPU, and where detection finds it.
//!
//! `sdkmanager` asks you to accept each licence. MaxHQ answers its
//! prompts one at a time, yes only to a licence whose ID and text you
//! accepted, as [`Licences`] records. Any other licence makes the install
//! wait, showing its text, until you accept it.

mod licences;
mod sdkmanager;

use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::hosts::PendingLicence;
use maxhq_protocol::state::{HelperTool, ToolKind, ToolPlace};

pub use licences::Licences;

use super::defs;
use super::install::{InstallError, Installer, Layout};

/// The JDK, as the installer knows it.
pub const JDK: ToolKind = ToolKind::Helper {
    tool: HelperTool::Jdk,
};

/// The Android SDK, as the installer knows it.
pub const SDK: ToolKind = ToolKind::Helper {
    tool: HelperTool::AndroidSdk,
};

/// The oldest Java the command-line tools run on.
const JAVA_MIN: u32 = 17;

/// The API level of the system image MaxHQ installs.
const IMAGE_PLATFORM: &str = "android-36";

/// How the Android SDK installs on this device.
#[derive(Clone, Debug)]
pub struct Sdk {
    /// Whether this device is a Mac, the only place the SDK installs.
    pub on_mac: bool,
    /// The file that records the licences you accepted, or `None` when
    /// maxd has no data folder.
    pub licences: Option<PathBuf>,
    /// The Java homes to try, in order.
    pub javas: Vec<PathBuf>,
    /// Whether to ask macOS's `java_home` for a Java after them.
    pub ask_macos: bool,
    /// How long one `sdkmanager` run may take.
    pub timeout: Duration,
}

impl Sdk {
    /// This device's, for the user whose home is `home`: the Java maxd's
    /// `JAVA_HOME` names, then Android Studio's and MaxHQ's own.
    pub fn for_host(home: Option<&str>) -> Self {
        let mut javas: Vec<PathBuf> = std::env::var_os("JAVA_HOME")
            .filter(|java| !java.is_empty())
            .map(PathBuf::from)
            .into_iter()
            .collect();
        javas.extend(
            home.map(|home| bundled_javas(Path::new(home)))
                .unwrap_or_default(),
        );
        Self {
            on_mac: cfg!(target_os = "macos"),
            licences: maxhq_core::paths::data_dir().map(|dir| dir.join("licences.json")),
            javas,
            ask_macos: cfg!(target_os = "macos"),
            timeout: Duration::from_secs(60 * 60),
        }
    }

    /// The record of the licences you accepted.
    pub fn record(&self) -> Result<Licences, String> {
        self.licences
            .clone()
            .map(Licences::at)
            .ok_or_else(|| "maxd has no data folder to record licences in.".to_owned())
    }
}

/// Why MaxHQ won't install `kind` here, when it's the JDK or the Android
/// SDK and this isn't a Mac.
pub fn refusal(kind: &ToolKind, workspace: bool, on_mac: bool) -> Option<String> {
    if *kind != JDK && *kind != SDK {
        return None;
    }
    if workspace {
        return Some("The Android SDK goes on the Mac, not in a workspace.".to_owned());
    }
    (!on_mac).then(|| "MaxHQ installs the Android SDK on a Mac only.".to_owned())
}

/// The SDK's folder in `layout`, beside the tools the installer keeps.
pub fn sdk_root(layout: &Layout) -> String {
    format!("{}/android", layout.tools())
}

/// The home of the JDK MaxHQ installed in `layout`.
pub fn jdk_home(layout: &Layout) -> String {
    format!("{}/current", layout.dir(&JDK))
}

/// The SDK folder in the home `home`, as detection looks for it.
pub fn sdk_root_in(home: &Path) -> PathBuf {
    PathBuf::from(sdk_root(&Layout::host(&home.to_string_lossy())))
}

/// The Javas that come with Android Studio, and the one MaxHQ installed,
/// for the user whose home is `home`.
pub fn bundled_javas(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Android Studio.app/Contents/jbr/Contents/Home"),
        home.join("Applications/Android Studio.app/Contents/jbr/Contents/Home"),
        PathBuf::from("/opt/android-studio/jbr"),
        home.join("android-studio/jbr"),
        PathBuf::from(jdk_home(&Layout::host(&home.to_string_lossy()))),
    ]
}

/// The packages `sdkmanager` installs for a device of `target`: the
/// emulator's tools and one system image for its CPU.
pub fn packages(target: &str) -> Vec<String> {
    let abi = if target.starts_with("aarch64") {
        "arm64-v8a"
    } else {
        "x86_64"
    };
    vec![
        "platform-tools".to_owned(),
        "emulator".to_owned(),
        format!("system-images;{IMAGE_PLATFORM};google_apis;{abi}"),
    ]
}

/// Installs the Android SDK in `layout`. It answers
/// `Ok(Some(pending))` when it stopped at licences you haven't accepted,
/// and `Ok(None)` once the SDK is whole. `say` hears each step.
pub async fn install(
    sdk: &Sdk,
    installer: &dyn Installer,
    layout: &Layout,
    say: &(dyn Fn(String) + Sync),
) -> Result<Option<Vec<PendingLicence>>, InstallError> {
    let record = sdk.record().map_err(InstallError::Failed)?;
    let java = match find_java(sdk).await {
        Some(java) => java,
        None => {
            say("Installing Java for the Android SDK".to_owned());
            let def = defs::plan(&JDK, false).ok_or(InstallError::NotBuilt)?;
            installer
                .ensure(&def, defs::pin(&JDK, false), &ToolPlace::Host)
                .await?;
            PathBuf::from(jdk_home(layout))
        }
    };
    say("Installing the Android command-line tools".to_owned());
    let def = defs::plan(&SDK, false).ok_or(InstallError::NotBuilt)?;
    installer
        .ensure(&def, defs::pin(&SDK, false), &ToolPlace::Host)
        .await?;
    let root = sdk_root(layout);
    link_tools(&root, &layout.dir(&SDK)).map_err(|err| {
        InstallError::Failed(format!("MaxHQ couldn't set up the SDK folder: {err}"))
    })?;

    let run = |args: Vec<String>| {
        let accepted = record.read();
        let java = java.clone();
        let root = root.clone();
        async move {
            sdkmanager::run(sdkmanager::Run {
                program: format!("{root}/cmdline-tools/latest/bin/sdkmanager"),
                root,
                java_home: java,
                args,
                accepted,
                timeout: sdk.timeout,
                progress: &|percent| say(format!("Installing Android SDK packages, {percent}%")),
            })
            .await
        }
    };
    say("Reading the Android SDK's licences".to_owned());
    let ran = run(vec!["--licenses".to_owned()]).await?;
    if !ran.declined.is_empty() {
        return Ok(Some(ran.declined));
    }
    say("Installing the emulator and a system image".to_owned());
    let mut args = vec!["--install".to_owned()];
    args.extend(packages(&layout.target));
    let ran = run(args).await?;
    if !ran.declined.is_empty() {
        return Ok(Some(ran.declined));
    }
    let whole = ["platform-tools/adb", "emulator/emulator"]
        .iter()
        .all(|program| Path::new(&root).join(program).is_file());
    if !ran.success || !whole {
        return Err(InstallError::Failed(format!(
            "sdkmanager didn't install the emulator: {}",
            ran.tail
        )));
    }
    Ok(None)
}

/// Points the SDK's `cmdline-tools/latest` at the installer's current
/// command-line tools, in `dir`.
fn link_tools(root: &str, dir: &str) -> std::io::Result<()> {
    let tools = Path::new(root).join("cmdline-tools");
    std::fs::create_dir_all(&tools)?;
    let latest = tools.join("latest");
    let target = PathBuf::from(format!("{dir}/current"));
    if std::fs::read_link(&latest).ok().as_ref() == Some(&target) {
        return Ok(());
    }
    let new = tools.join(".latest.new");
    let _ = std::fs::remove_file(&new);
    std::os::unix::fs::symlink(&target, &new)?;
    if latest.is_dir() && !latest.is_symlink() {
        std::fs::remove_dir_all(&latest)?;
    }
    std::fs::rename(&new, &latest)
}

/// The first of [`Sdk::javas`] new enough for the command-line tools, or
/// else what macOS's `java_home` names.
async fn find_java(sdk: &Sdk) -> Option<PathBuf> {
    for java in &sdk.javas {
        if new_enough(java).await {
            return Some(java.clone());
        }
    }
    if !sdk.ask_macos {
        return None;
    }
    let java = macos_java().await?;
    new_enough(&java).await.then_some(java)
}

/// Whether the Java in `java_home` runs and is new enough.
async fn new_enough(java_home: &Path) -> bool {
    java_major(java_home)
        .await
        .is_some_and(|major| major >= JAVA_MIN)
}

/// What macOS's `java_home` names for a Java new enough, if any.
async fn macos_java() -> Option<PathBuf> {
    let min = format!("{JAVA_MIN}+");
    let output = quick(Path::new("/usr/libexec/java_home"), &["-v", &min]).await?;
    let path = PathBuf::from(output.trim());
    path.is_dir().then_some(path)
}

/// The major version of the Java in `java_home`, if it runs.
async fn java_major(java_home: &Path) -> Option<u32> {
    let java = java_home.join("bin").join("java");
    if !java.is_file() {
        return None;
    }
    java_version(&quick(&java, &["-version"]).await?)
}

/// The major version in what `java -version` prints, where an old Java
/// says `1.8.0` for 8.
fn java_version(text: &str) -> Option<u32> {
    let quoted = text.split_once("version \"")?.1.split('"').next()?;
    let mut parts = quoted.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Runs `program` briefly and gives what it printed on both streams.
async fn quick(program: &Path, args: &[&str]) -> Option<String> {
    let mut command = super::HostExec.command(&program.to_string_lossy()).ok()?;
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(text)
}

#[cfg(test)]
mod tests;
