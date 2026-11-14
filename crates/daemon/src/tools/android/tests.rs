// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Android SDK's tests. The installer is a fake that puts a fake
//! `sdkmanager` and a fake Java in a temp home, so nothing reaches Google
//! or a JDK vendor. The fake `sdkmanager` asks for the licences in its
//! fixture folder the way the real one does, and logs each answer.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::{
    DeviceTools, Entity, EntityKey, ToolInfo, ToolKind, ToolPlace, ToolState,
};

use super::*;
use crate::testing::TempDir;
use crate::tools::install::{InstallDef, Installed};
use crate::tools::Tools;

/// The fake `sdkmanager`. Its fixture folder sits beside its versions,
/// holding its licences in `licences/<id>` and its log in `log`.
const SDKMANAGER: &str = r#"#!/bin/sh
fx="$(dirname "$0")/../../fixture"
root=; mode=; pkgs=
for a in "$@"; do
  case "$a" in
    --sdk_root=*) root="${a#--sdk_root=}" ;;
    --licenses) mode=licenses ;;
    --install) mode=install ;;
    *) pkgs="$pkgs $a" ;;
  esac
done
echo "run $mode java=$JAVA_HOME home=$ANDROID_HOME$pkgs" >> "$fx/log"
mkdir -p "$root/licenses"
unaccepted() {
  for f in "$fx"/licences/*; do
    id="${f##*/}"
    [ "$(cat "$root/licenses/$id" 2>/dev/null)" = "$(cksum < "$f")" ] || echo "$id"
  done
}
ask() {
  printf 'License %s:\n---------------------------------------\n' "$1"
  cat "$fx/licences/$1"
  printf '\n---------------------------------------\nAccept? (y/N): '
  read ans
  echo "answer $1 $ans" >> "$fx/log"
  if [ "$ans" = y ]; then cksum < "$fx/licences/$1" > "$root/licenses/$1"; fi
}
todo="$(unaccepted)"
if [ "$mode" = licenses ]; then
  if [ -n "$todo" ]; then
    printf 'Some SDK package licenses not accepted.\nReview licenses that have not been accepted (y/N)? '
    read ans
    echo "review $ans" >> "$fx/log"
    [ "$ans" = y ] || exit 0
    for id in $todo; do ask "$id"; done
  fi
  exit 0
fi
for id in $todo; do ask "$id"; done
if [ -n "$(unaccepted)" ]; then echo "Skipping packages whose license isn't accepted"; exit 1; fi
printf '[=====     ] 50%% Downloading\r[==========] 100%% Done\n'
mkdir -p "$root/platform-tools" "$root/emulator"
printf '#!/bin/sh\n' > "$root/platform-tools/adb"
printf '#!/bin/sh\n' > "$root/emulator/emulator"
chmod 755 "$root/platform-tools/adb" "$root/emulator/emulator"
for p in $pkgs; do
  case "$p" in
    system-images*)
      d="$root/$(printf '%s' "$p" | tr ';' '/')"
      mkdir -p "$d"
      echo "Pkg.Revision=1" > "$d/source.properties"
      ;;
  esac
done
"#;

/// A Java that says it's `version`.
fn java(version: &str) -> String {
    format!("#!/bin/sh\necho 'openjdk version \"{version}\" 2025-10-21' >&2\n")
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A home, with what the installer would put there.
struct Home {
    dir: TempDir,
    layout: Layout,
}

impl Home {
    fn new() -> Self {
        let dir = TempDir::new();
        let layout = Layout::host(&dir.path().to_string_lossy());
        Self { dir, layout }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// The fixture folder the fake `sdkmanager` reads.
    fn fixture(&self) -> PathBuf {
        PathBuf::from(format!("{}/android-sdk/fixture", self.layout.tools()))
    }

    fn licence(&self, id: &str, text: &str) {
        let path = self.fixture().join("licences").join(id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn log(&self) -> Vec<String> {
        fs::read_to_string(self.fixture().join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn sdk(&self, javas: Vec<PathBuf>) -> Sdk {
        Sdk {
            on_mac: true,
            licences: Some(self.path("data/licences.json")),
            javas,
            ask_macos: false,
            timeout: Duration::from_secs(30),
        }
    }

    /// A Java of `version` outside the installer's folders.
    fn own_java(&self, version: &str) -> PathBuf {
        let home = self.path(&format!("java-{version}"));
        write(&home.join("bin/java"), &java(version));
        home
    }
}

/// An installer that puts the fake `sdkmanager` and a fake JDK where the
/// real one would, and says what it was asked.
struct Placed {
    layout: Layout,
    asked: Mutex<Vec<ToolKind>>,
}

impl Placed {
    fn new(home: &Home) -> Arc<Self> {
        Arc::new(Self {
            layout: home.layout.clone(),
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<ToolKind> {
        self.asked.lock().unwrap().clone()
    }
}

impl Installer for Placed {
    fn ensure<'a>(
        &'a self,
        def: &'a InstallDef,
        version: Option<&'a str>,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<Installed, InstallError>> {
        assert_eq!(*place, ToolPlace::Host);
        self.asked.lock().unwrap().push(def.kind.clone());
        let version = version.unwrap_or("1").to_owned();
        let dir = self.layout.dir(&def.kind);
        let root = format!("{dir}/{version}");
        let (entry, text) = if def.kind == SDK {
            ("bin/sdkmanager", SDKMANAGER.to_owned())
        } else {
            ("bin/java", java("21.0.13"))
        };
        write(&Path::new(&root).join(entry), &text);
        fs::write(
            format!("{root}/.install-complete"),
            format!("version {version}\nentry {entry}\n"),
        )
        .unwrap();
        let current = format!("{dir}/current");
        let _ = fs::remove_file(&current);
        std::os::unix::fs::symlink(&version, &current).unwrap();
        async move {
            Ok(Installed {
                version,
                path: format!("{current}/{entry}"),
                root,
            })
        }
        .boxed()
    }

    fn remove<'a>(
        &'a self,
        kind: &'a ToolKind,
        _place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), InstallError>> {
        let _ = fs::remove_dir_all(self.layout.dir(kind));
        async { Ok(()) }.boxed()
    }
}

/// Tools for `home`, installing through `placed`.
fn tools(home: &Home, sdk: Sdk, placed: Arc<Placed>) -> Arc<Tools> {
    let mut tools = Tools::new(
        crate::secrets::testing::hub(),
        None,
        Arc::new(crate::tools::Npm::new()),
        Some(home.path("bin").into_os_string()),
    );
    tools.home = Some(home.dir.path().to_string_lossy().into_owned());
    tools.android = sdk;
    let tools = Arc::new(tools);
    tools.set_installer(placed);
    tools
}

fn published(tools: &Tools) -> DeviceTools {
    match tools
        .hub
        .get(&EntityKey::DeviceTools(tools.hub.device().id))
    {
        Some(Entity::DeviceTools(found)) => found,
        other => panic!("no tools entity: {other:?}"),
    }
}

/// Waits until the host's SDK entry passes `done`, and gives it.
async fn until(tools: &Tools, done: impl Fn(&ToolInfo) -> bool) -> ToolInfo {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let found = published(tools);
        if let Some(info) = found.tools.iter().find(|info| info.kind == SDK)
            && done(info)
        {
            return info.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "{found:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn pending(info: &ToolInfo) -> Vec<PendingLicence> {
    match &info.state {
        ToolState::Licences { pending } => pending.clone(),
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn the_sdk_waits_on_each_licence_and_says_yes_only_to_those_accepted() {
    let home = Home::new();
    home.licence("android-sdk-license", "Terms and Conditions\n\nOne.");
    home.licence("android-sdk-preview-license", "Preview terms.");
    let java = home.own_java("17.0.2");
    let placed = Placed::new(&home);
    let tools = tools(&home, home.sdk(vec![java.clone()]), placed.clone());

    assert_eq!(
        tools.install(SDK, ToolPlace::Host, None),
        ActionOutcome::Done
    );
    let waiting = until(&tools, |info| !pending(info).is_empty()).await;
    let shown = pending(&waiting);
    let ids: Vec<&str> = shown.iter().map(|licence| licence.id.as_str()).collect();
    assert_eq!(ids, ["android-sdk-license", "android-sdk-preview-license"]);
    assert_eq!(shown[0].text, "Terms and Conditions\n\nOne.");
    // The Java found was new enough, so no JDK was installed.
    assert_eq!(placed.asked(), [SDK]);
    let root = sdk_root(&home.layout);
    assert_eq!(
        home.log(),
        [
            format!("run licenses java={} home={root}", java.display()),
            "review y".to_owned(),
            "answer android-sdk-license n".to_owned(),
            "answer android-sdk-preview-license n".to_owned(),
        ]
    );

    // A text you didn't read isn't accepted, nor a licence nothing waits on.
    let changed = tools.accept_licence("android-sdk-license", &"0".repeat(64));
    assert!(
        matches!(&changed, ActionOutcome::Refused { reason } if reason.contains("changed")),
        "{changed:?}"
    );
    let unknown = tools.accept_licence("someone-elses-license", &shown[0].text_sha256);
    assert!(
        matches!(&unknown, ActionOutcome::Refused { reason } if reason.contains("No install")),
        "{unknown:?}"
    );

    // One accepted, and it still waits on the other.
    assert_eq!(
        tools.accept_licence(&shown[0].id, &shown[0].text_sha256),
        ActionOutcome::Done
    );
    let left = pending(&until(&tools, |info| pending(info).len() == 1).await);
    assert_eq!(left, [shown[1].clone()]);
    let recorded = home.sdk(Vec::new()).record().unwrap().read();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].id, "android-sdk-license");
    assert_eq!(recorded[0].text_sha256, shown[0].text_sha256);
    assert_eq!(recorded[0].accepted_on.len(), "2026-11-02".len());

    // The last one goes on with the install.
    assert_eq!(
        tools.accept_licence(&shown[1].id, &shown[1].text_sha256),
        ActionOutcome::Done
    );
    let installed = until(&tools, |info| info.state == ToolState::Idle).await;
    assert_eq!(installed.path.as_deref(), Some(root.as_str()));
    assert_eq!(
        installed.version.as_deref(),
        Some(defs::ANDROID_TOOLS_VERSION)
    );
    let abi = if home.layout.target.starts_with("aarch64") {
        "arm64-v8a"
    } else {
        "x86_64"
    };
    for program in ["platform-tools/adb", "emulator/emulator"] {
        assert!(Path::new(&root).join(program).is_file(), "{program}");
    }
    let image = format!("{root}/system-images/android-36/google_apis/{abi}/source.properties");
    assert!(Path::new(&image).is_file());
    assert!(Path::new(&format!("{root}/cmdline-tools/latest/bin/sdkmanager")).is_file());
    assert_eq!(sdk_root_in(home.dir.path()), PathBuf::from(&root));
    let log = home.log();
    let yes: Vec<&String> = log.iter().filter(|line| line.ends_with(" y")).collect();
    assert_eq!(
        yes,
        [
            "review y",
            "review y",
            "answer android-sdk-license y",
            "answer android-sdk-preview-license y",
        ]
    );
    assert!(
        log.last().unwrap().starts_with("run install") && log.last().unwrap().contains(abi),
        "{log:?}"
    );

    // A changed licence asks again, with its new text.
    home.licence("android-sdk-preview-license", "Preview terms, changed.");
    assert_eq!(
        tools.install(SDK, ToolPlace::Host, None),
        ActionOutcome::Done
    );
    let again = pending(&until(&tools, |info| !pending(info).is_empty()).await);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].text, "Preview terms, changed.");
    assert_ne!(again[0].text_sha256, shown[1].text_sha256);
    assert_eq!(
        home.log().last().map(String::as_str),
        Some("answer android-sdk-preview-license n")
    );

    // Removing the SDK takes its packages too.
    assert_eq!(tools.remove(SDK, ToolPlace::Host), ActionOutcome::Done);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while Path::new(&root).exists() || published(&tools).tools.iter().any(|info| info.kind == SDK) {
        assert!(tokio::time::Instant::now() < deadline, "{root} stayed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_jdk_is_installed_only_when_none_is_new_enough() {
    let home = Home::new();
    home.licence("android-sdk-license", "Terms.");
    let old = home.own_java("1.8.0_392");
    let sdk = home.sdk(vec![old, PathBuf::from(jdk_home(&home.layout))]);
    let placed = Placed::new(&home);
    let quiet = |_: String| {};

    let waits = install(&sdk, &*placed, &home.layout, &quiet).await.unwrap();
    assert_eq!(waits.map(|pending| pending.len()), Some(1));
    assert_eq!(placed.asked(), [JDK, SDK]);
    assert_eq!(
        home.log()[0],
        format!(
            "run licenses java={} home={}",
            jdk_home(&home.layout),
            sdk_root(&home.layout)
        )
    );

    // Next time MaxHQ's own JDK is found.
    install(&sdk, &*placed, &home.layout, &quiet).await.unwrap();
    assert_eq!(placed.asked(), [JDK, SDK, SDK]);
}

#[tokio::test]
async fn the_sdk_and_its_jdk_install_on_a_mac_only() {
    let home = Home::new();
    let mut sdk = home.sdk(Vec::new());
    sdk.on_mac = false;
    let placed = Placed::new(&home);
    let tools = tools(&home, sdk, placed.clone());
    for kind in [SDK, JDK] {
        assert_eq!(
            tools.install(kind, ToolPlace::Host, None),
            ActionOutcome::Refused {
                reason: "MaxHQ installs the Android SDK on a Mac only.".into()
            }
        );
    }
    assert_eq!(
        refusal(&SDK, true, true),
        Some("The Android SDK goes on the Mac, not in a workspace.".into())
    );
    assert_eq!(refusal(&JDK, false, true), None);
    assert!(placed.asked().is_empty());
}

#[test]
fn java_versions_read_plainly() {
    assert_eq!(
        java_version("openjdk version \"21.0.13\" 2025-10-21"),
        Some(21)
    );
    assert_eq!(java_version("java version \"1.8.0_392\""), Some(8));
    assert_eq!(java_version("openjdk version \"17\" 2021-09-14"), Some(17));
    assert_eq!(java_version("no Java here"), None);
    assert_eq!(
        packages("aarch64-apple-darwin")[2],
        "system-images;android-36;google_apis;arm64-v8a"
    );
    assert_eq!(
        packages("x86_64-apple-darwin")[2],
        "system-images;android-36;google_apis;x86_64"
    );
}
