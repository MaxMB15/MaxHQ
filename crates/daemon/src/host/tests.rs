// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The host check and apply against a temp root, the Incus fake and a fake
//! command runner. Nothing here reads the real `/etc`, systemd or nft.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use maxhq_core::host_spec::{self, HostSpec};
use maxhq_protocol::hosts::{HostFix, HostItem, HostItemState, HostSource};
use maxhq_protocol::ids::DeviceId;

use super::system::{self, Step, MARKER, NIXOS};
use super::*;
use crate::incus::fake::{FakeIncus, State};
use crate::service::{Manager, Ran, Runner};
use crate::testing::TempDir;

/// The tracked example spec.
const EXAMPLE: &str = include_str!("../../../../nix/maxhq-host.example.toml");

const BOOKWORM: &str = "ID=debian\nVERSION_ID=\"12\"\nVERSION_CODENAME=bookworm\n";
const NOBLE: &str = "ID=ubuntu\nVERSION_ID=\"24.04\"\nVERSION_CODENAME=noble\n";

/// A device under a temp root, as `dev`.
struct Host {
    dir: TempDir,
}

impl Host {
    fn new(release: &str) -> Self {
        let host = Self {
            dir: TempDir::new(),
        };
        host.write("etc/os-release", release);
        host.write("etc/group", "incus-admin:x:990:\nusers:x:100:dev\n");
        host.write("proc/swaps", "Filename Type Size Used Priority\n");
        host.write(
            "proc/self/mountinfo",
            "22 1 0:21 / / rw - ext4 /dev/sda1 rw\n",
        );
        fs::create_dir_all(host.path("sys/fs/cgroup/system.slice")).unwrap();
        host.write("sys/fs/cgroup/system.slice/memory.low", "0\n");
        host
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel.trim_start_matches('/'))
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn home(&self) -> PathBuf {
        self.path("home/dev")
    }

    /// Puts the spec where the user's goes.
    fn spec(&self, text: &str) {
        let path = spec_path(&Os::Nix, &self.home());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// The env the check reads through. `tools` are the programs found.
    fn env(&self, incus: Option<&FakeIncus>, tools: &[&str]) -> Env {
        let found: BTreeSet<String> = tools.iter().map(|t| (*t).to_owned()).collect();
        let home = self.home();
        let os = Os::from_release(&self.read("etc/os-release"));
        Env {
            root: self.root().to_owned(),
            os,
            user: User {
                name: "dev".to_owned(),
                uid: rustix::process::getuid().as_raw(),
                gid: rustix::process::getgid().as_raw(),
                home: home.clone(),
            },
            maxd: PathBuf::from("/home/dev/.local/bin/maxd"),
            incus: incus.map(|fake| Incus::new(&fake.socket)),
            service: Some(Setup {
                manager: Manager::Systemd,
                config_home: home.join(".config"),
                home,
                uid: 1000,
                exe: PathBuf::from("/home/dev/.local/bin/maxd"),
                path: "/usr/bin:/bin".to_owned(),
                left_out: Vec::new(),
                socket: None,
                root: self.root().to_owned(),
                data: None,
                runtime: None,
                app: None,
            }),
            find: Box::new(move |name| found.contains(name).then(|| PathBuf::from(name))),
        }
    }
}

fn item<'a>(items: &'a [HostItem], id: &str) -> &'a HostItem {
    items
        .iter()
        .find(|item| item.id == id)
        .unwrap_or_else(|| panic!("no item {id} in {items:#?}"))
}

fn ids(items: &[HostItem]) -> Vec<&str> {
    items.iter().map(|item| item.id.as_str()).collect()
}

fn root_fix() -> Option<HostFix> {
    Some(HostFix::RootCommand {
        command: "sudo /home/dev/.local/bin/maxd host apply --system".to_owned(),
    })
}

/// Runs commands against the temp root: packages, groups, linger and
/// units change there, and every call is kept.
struct FakeExec {
    root: PathBuf,
    packages: BTreeSet<String>,
    up: BTreeSet<String>,
    calls: Vec<String>,
    /// A command that fails, by its start.
    fails: Option<String>,
    /// What `gpg --show-keys --with-colons` prints for any key.
    gpg: String,
}

/// What gpg prints for Zabbly's key, its primary key and a subkey.
const ZABBLY_COLONS: &str = "pub:-:3072:1:82CC8797C838DCFD:1677868800:::-:::scESC::::::23::0:
fpr:::::::::4EFC590696CB15B87C73A3AD82CC8797C838DCFD:
uid:-::::1677868800::0000000000000000000000000000000000000000::Zabbly Kernel Builds <info@example.com>::::::::::0:
sub:-:3072:1:0000000000000001:1677868800::::::e::::::23:
fpr:::::::::0000000000000000000000000000000000000001:
";

impl FakeExec {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            packages: BTreeSet::new(),
            up: BTreeSet::new(),
            calls: Vec::new(),
            fails: None,
            gpg: ZABBLY_COLONS.to_owned(),
        }
    }

    /// The calls that change something.
    fn changes(&self) -> Vec<&str> {
        self.calls
            .iter()
            .map(String::as_str)
            .filter(|call| {
                !call.starts_with("dpkg-query")
                    && !call.starts_with("systemctl is-")
                    && !call.starts_with("gpg --show-keys")
            })
            .collect()
    }

    fn file(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }
}

impl Runner for FakeExec {
    fn run(&mut self, program: &str, args: &[&str]) -> io::Result<Ran> {
        let call = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.calls.push(call.clone());
        let ok = |stdout: &str| {
            Ok(Ran {
                success: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        };
        let no = || {
            Ok(Ran {
                success: false,
                stdout: String::new(),
                stderr: "no".to_owned(),
            })
        };
        if self
            .fails
            .as_deref()
            .is_some_and(|start| call.starts_with(start))
        {
            return no();
        }
        match (program, args) {
            ("dpkg-query", [.., package]) => {
                if self.packages.contains(*package) {
                    ok("install ok installed")
                } else {
                    no()
                }
            }
            ("env", [_, "apt-get", "install", _, _, packages @ ..]) => {
                self.packages
                    .extend(packages.iter().map(|p| (*p).to_owned()));
                ok("")
            }
            ("usermod", ["-aG", group, user]) => {
                let path = self.file("/etc/group");
                let text = fs::read_to_string(&path).unwrap();
                let text =
                    text.replace(&format!("{group}:x:990:"), &format!("{group}:x:990:{user}"));
                fs::write(path, text).unwrap();
                ok("")
            }
            ("loginctl", ["enable-linger", user]) => {
                let path = self.file(&format!("/var/lib/systemd/linger/{user}"));
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "").unwrap();
                ok("")
            }
            ("curl", [.., key, _]) => {
                let path = self.file(key);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "key").unwrap();
                ok("")
            }
            ("gpg", ["--show-keys", "--with-colons", _]) => {
                let colons = self.gpg.clone();
                ok(&colons)
            }
            ("systemctl", ["is-enabled" | "is-active", "--quiet", name]) => {
                if self.up.contains(*name) {
                    ok("")
                } else {
                    no()
                }
            }
            ("systemctl", ["enable", "--now", name] | ["restart", name]) => {
                if name.starts_with("systemd-zram-setup") {
                    let swaps = self.file("/proc/swaps");
                    fs::write(swaps, "Filename\n/dev/zram0 partition 8G 0 100\n").unwrap();
                }
                self.up.insert((*name).to_owned());
                ok("")
            }
            ("systemctl", ["set-property", "--runtime", "system.slice", property]) => {
                let bytes = property.trim_start_matches("MemoryLow=");
                fs::write(self.file("/sys/fs/cgroup/system.slice/memory.low"), bytes).unwrap();
                ok("")
            }
            _ => ok(""),
        }
    }
}

#[test]
fn releases_name_the_system() {
    assert_eq!(
        Os::from_release("ID=nixos\nVERSION_ID=\"25.05\"\n"),
        Os::Nix
    );
    assert_eq!(Os::from_release("ID=arch\n"), Os::Other("arch".into()));
    let bookworm = Os::from_release(BOOKWORM);
    assert_eq!(
        bookworm,
        Os::Apt {
            id: "debian".into(),
            version: Some("12".into()),
            codename: Some("bookworm".into()),
        }
    );
    assert!(!bookworm.packages_incus());
    assert!(Os::from_release(NOBLE).packages_incus());
    assert!(Os::from_release("ID=debian\nVERSION_ID=13\n").packages_incus());
    assert!(!Os::from_release("ID=ubuntu\nVERSION_ID=22.04\n").packages_incus());
    assert!(Os::from_release("ID=debian\n").packages_incus());
    assert!(!Os::Mac.is_linux());
}

#[test]
fn users_come_from_passwd() {
    let host = Host::new(BOOKWORM);
    host.write(
        "etc/passwd",
        "root:x:0:0:root:/root:/bin/sh\ndev:x:1000:100::/home/dev:/bin/bash\n",
    );
    let dev = User::lookup(host.root(), "dev").unwrap();
    assert_eq!((dev.uid, dev.gid), (1000, 100));
    assert_eq!(dev.home, PathBuf::from("/home/dev"));
    assert_eq!(User::lookup(host.root(), "nobody"), None);
}

#[tokio::test]
async fn a_bare_debian_host_offers_the_root_command() {
    let host = Host::new(BOOKWORM);
    host.spec(EXAMPLE);
    let env = host.env(None, &["claude"]);
    let report = report(&env, DeviceId::derive("devbox")).await.unwrap();
    assert!(matches!(report.source, HostSource::File { .. }));
    let items = &report.items;
    assert_eq!(item(items, "service.user").state, HostItemState::Ok);
    for id in [
        "incus.group",
        "incus",
        "caps.parent",
        "zram",
        "linger",
        "units.prepare",
        "units.shutdown",
        "egress",
        "data_dir",
    ] {
        let found = item(items, id);
        assert_eq!(found.state, HostItemState::Missing, "{id}");
        assert_eq!(found.fix, root_fix(), "{id}");
    }
    // Incus isn't there, so nothing can say about its pool or bridge.
    for id in ["incus.pool", "incus.bridge"] {
        assert!(matches!(
            item(items, id).state,
            HostItemState::Unknown { .. }
        ));
    }
    let mount = item(items, "incus.pool.mount");
    assert_eq!((&mount.state, &mount.fix), (&HostItemState::Missing, &None));
    assert_eq!(
        item(items, "caps.system_memory_low").state,
        HostItemState::Drift {
            want: "4GiB".into(),
            have: "none".into(),
        }
    );
    let service = item(items, "service");
    assert_eq!(
        (&service.state, &service.fix),
        (&HostItemState::Missing, &Some(HostFix::Now))
    );
    assert_eq!(item(items, "tools.claude").state, HostItemState::Ok);
    let cursor = item(items, "tools.cursor");
    assert_eq!(
        (&cursor.state, &cursor.fix),
        (&HostItemState::Missing, &None)
    );
    assert_eq!(item(items, "tools.system.git").fix, root_fix());
    assert!(!all_ok(&report));
    // No `kvm` item, since the Android SDK lives on the Mac.
    assert!(!ids(items).iter().any(|id| id.contains("kvm")));
}

#[tokio::test]
async fn incus_says_how_the_pool_and_bridge_stand() {
    let host = Host::new(NOBLE);
    host.spec(EXAMPLE);
    let fake = FakeIncus::start(State::default());
    let env = host.env(Some(&fake), &[]);
    let items = check::items(&env, &HostSpec::from_toml(EXAMPLE).unwrap()).await;
    assert_eq!(item(&items, "incus").state, HostItemState::Ok);
    // The recorded `agents` pool is btrfs, as the spec wants.
    assert_eq!(item(&items, "incus.pool").state, HostItemState::Ok);
    let bridge = item(&items, "incus.bridge");
    assert_eq!(
        bridge.state,
        HostItemState::Drift {
            want: "10.10.0.1/24".into(),
            have: "10.100.2.1/24".into(),
        }
    );
    assert_eq!(bridge.fix, None);

    let fresh = EXAMPLE
        .replace("name = \"agents\"", "name = \"fresh\"")
        .replace("name = \"agentbr0\"", "name = \"freshbr0\"");
    let spec = HostSpec::from_toml(&fresh).unwrap();
    let items = check::items(&env, &spec).await;
    for id in ["incus.pool", "incus.bridge"] {
        let found = item(&items, id);
        assert_eq!(
            (&found.state, &found.fix),
            (&HostItemState::Missing, &Some(HostFix::Now))
        );
    }

    // Only these two, since the service fix would run systemctl.
    let asked = vec!["incus.pool".to_owned(), "incus.bridge".to_owned()];
    let done = apply_user(&env, &spec, &items, Some(&asked), Restart::Later).await;
    assert_eq!(
        done.unwrap(),
        ["Made storage pool fresh.", "Made bridge freshbr0."]
    );
    let writes: Vec<_> = fake
        .state()
        .writes()
        .into_iter()
        .map(|logged| (logged.target.clone(), logged.body.clone()))
        .collect();
    assert_eq!(writes.len(), 2, "{writes:?}");
    assert_eq!(writes[0].0, "/1.0/storage-pools");
    assert_eq!(writes[0].1["name"], "fresh");
    assert_eq!(writes[0].1["driver"], "btrfs");
    assert_eq!(writes[0].1["config"]["source"], "/srv/agents.img");
    assert_eq!(writes[0].1["config"]["size"], (100u64 << 30).to_string());
    assert_eq!(writes[1].0, "/1.0/networks");
    assert_eq!(writes[1].1["name"], "freshbr0");
    assert_eq!(writes[1].1["type"], "bridge");
    assert_eq!(writes[1].1["config"]["ipv4.address"], "10.10.0.1/24");
    assert_eq!(writes[1].1["config"]["ipv4.nat"], "true");
}

#[tokio::test]
async fn the_user_part_fixes_only_what_it_can() {
    let host = Host::new(NOBLE);
    let env = host.env(None, &[]);
    let spec = HostSpec::from_toml(EXAMPLE).unwrap();
    let items = check::items(&env, &spec).await;
    let asked = vec!["linger".to_owned()];
    let err = apply_user(&env, &spec, &items, Some(&asked), Restart::Later)
        .await
        .unwrap_err();
    assert!(err.contains("can't fix"), "{err}");
    let asked = vec!["nothing".to_owned()];
    let err = apply_user(&env, &spec, &items, Some(&asked), Restart::Later)
        .await
        .unwrap_err();
    assert!(err.contains("no item nothing"), "{err}");

    let mut root = host.env(None, &[]);
    root.user.uid = 0;
    let err = apply_user(&root, &spec, &items, None, Restart::Later)
        .await
        .unwrap_err();
    assert!(err.contains("not root"), "{err}");
}

#[tokio::test]
async fn cgroup_limits_read_as_people_say_them() {
    let host = Host::new(NOBLE);
    let cgroup = "sys/fs/cgroup/maxhq-workspaces";
    host.write(
        &format!("{cgroup}/memory.high"),
        &format!("{}\n", 12u64 << 30),
    );
    host.write(&format!("{cgroup}/cpu.max"), "400000 100000\n");
    host.write(&format!("{cgroup}/cpu.weight"), "50\n");
    host.write(
        "sys/fs/cgroup/system.slice/memory.low",
        &format!("{}\n", 4u64 << 30),
    );
    host.write("proc/swaps", "Filename\n/dev/zram0 partition 8G 0 100\n");
    let env = host.env(None, &[]);
    let items = check::items(&env, &HostSpec::from_toml(EXAMPLE).unwrap()).await;
    for id in [
        "caps.parent",
        "caps.memory_high",
        "caps.cpu_weight",
        "caps.system_memory_low",
        "zram",
    ] {
        let found = item(&items, id);
        assert_eq!(
            (&found.state, &found.fix),
            (&HostItemState::Ok, &None),
            "{id}"
        );
    }
    assert_eq!(
        item(&items, "caps.cpu_max").state,
        HostItemState::Drift {
            want: "8 CPUs".into(),
            have: "4 CPUs".into(),
        }
    );

    host.write(&format!("{cgroup}/memory.high"), "max\n");
    let items = check::items(&env, &HostSpec::from_toml(EXAMPLE).unwrap()).await;
    assert_eq!(
        item(&items, "caps.memory_high").state,
        HostItemState::Drift {
            want: "12GiB".into(),
            have: "no limit".into(),
        }
    );
}

#[tokio::test]
async fn a_nixos_host_offers_nixos_changes() {
    let host = Host::new("ID=nixos\n");
    let json = r#"{"service":{"user":"dev","data_dir":"/srv/maxhq"},"incus":{"bridge":{"name":"agentbr0"}},"caps":{"parent":"maxhq-workspaces","manage_parent":true,"system_memory_low":"4GiB","zram":true}}"#;
    host.write(NIX_SPEC, json);
    host.write(
        "etc/systemd/system/maxd-prepare.service",
        "made by the module",
    );
    let env = host.env(None, &[]);
    let report = report(&env, DeviceId::derive("devbox")).await.unwrap();
    assert_eq!(report.source, HostSource::Nix);
    let items = &report.items;
    // Each fix names the module option that renders the item.
    for (id, change) in [
        ("incus.group", "services.maxd.incusAdmin = true;"),
        ("incus", "services.maxd.host.incus.enable = true;"),
        (
            "caps.parent",
            "services.maxd.host.caps.manage_parent = true;",
        ),
        (
            "caps.system_memory_low",
            "services.maxd.host.caps.system_memory_low = \"4GiB\";",
        ),
        ("zram", "services.maxd.host.caps.zram = true;"),
        ("linger", "services.maxd.enable = true;"),
        ("units.shutdown", HOST_CHANGE),
        (
            "egress",
            "services.maxd.host.incus.bridge.name = \"agentbr0\";",
        ),
        (
            "data_dir",
            "services.maxd.host.service.data_dir = \"/srv/maxhq\";",
        ),
    ] {
        let want = Some(HostFix::NixChange {
            change: change.into(),
        });
        assert_eq!(item(items, id).fix, want, "{id}");
    }
    // The module writes its own unit, so being there is enough.
    assert_eq!(item(items, "units.prepare").state, HostItemState::Ok);
    let service = item(items, "service");
    assert_eq!(
        service.fix,
        Some(HostFix::NixChange {
            change: "services.maxd.enable = true;".into(),
        })
    );
}

#[tokio::test]
async fn the_modules_unit_owns_the_service_and_one_of_maxds_shadows_it() {
    let host = Host::new("ID=nixos\n");
    host.write("etc/systemd/user/maxd.service", "made by the module");
    let env = host.env(None, &[]);
    let spec = HostSpec::default();
    let items = check::items(&env, &spec).await;
    let service = item(&items, "service");
    assert_eq!((&service.state, &service.fix), (&HostItemState::Ok, &None));

    // An earlier `maxd service install` left a unit in the user's folder.
    host.write(
        "home/dev/.config/systemd/user/maxd.service",
        "an older install",
    );
    let shadow = host.home().join(".config/systemd/user/maxd.service");
    let items = check::items(&env, &spec).await;
    let service = item(&items, "service");
    let command =
        "/home/dev/.local/bin/maxd service uninstall && systemctl --user restart maxd.service";
    assert_eq!(
        service.state,
        HostItemState::Drift {
            want: "the NixOS module's unit alone".into(),
            have: format!(
                "{} too, which shadows it. Remove it with `{command}`",
                shadow.display()
            ),
        }
    );
    // It runs as you, so it's no root command, which the app would run
    // with sudo.
    assert_eq!(service.fix, None);
    // The check only reads. The shadow stays until you remove it.
    assert_eq!(
        host.read("home/dev/.config/systemd/user/maxd.service"),
        "an older install"
    );
}

#[tokio::test]
async fn apply_leaves_the_service_to_the_modules_unit() {
    let host = Host::new(NOBLE);
    let env = host.env(None, &[]);
    let spec = HostSpec::default();
    let items = check::items(&env, &spec).await;
    assert_eq!(item(&items, "service").fix, Some(HostFix::Now));

    // The module's unit turns up before the fix runs.
    host.write("etc/systemd/user/maxd.service", "made by the module");
    let asked = vec!["service".to_owned()];
    let done = apply_user(&env, &spec, &items, Some(&asked), Restart::Later)
        .await
        .unwrap();
    assert_eq!(
        done,
        ["The NixOS module owns maxd's unit, so this wrote nothing. Restart maxd with `systemctl --user restart maxd.service`."]
    );
    assert!(!host
        .path("home/dev/.config/systemd/user/maxd.service")
        .exists());
    let items = check::items(&env, &spec).await;
    assert_eq!(item(&items, "service").state, HostItemState::Ok);
}

#[tokio::test]
async fn a_nixos_host_without_the_module_spec_offers_the_spec() {
    let host = Host::new("ID=nixos\n");
    host.spec(EXAMPLE);
    let env = host.env(None, &[]);
    let report = report(&env, DeviceId::derive("devbox")).await.unwrap();
    assert!(matches!(report.source, HostSource::File { .. }));
    let change = Some(HostFix::NixChange {
        change: HOST_CHANGE.into(),
    });
    for id in [
        "incus.group",
        "zram",
        "linger",
        "units.prepare",
        "egress",
        "data_dir",
    ] {
        assert_eq!(item(&report.items, id).fix, change, "{id}");
    }
}

#[test]
fn render_prints_what_system_writes() {
    let host = Host::new(NOBLE);
    let env = host.env(None, &[]);
    let spec = HostSpec::from_toml(EXAMPLE).unwrap();
    let wants = system::files(&env, &spec);
    let text = |id: &str, n: usize| {
        wants.iter().find(|want| want.id == id).unwrap().files[n]
            .1
            .clone()
    };

    // The module's JSON renders the same as the TOML, and as maxd reads it.
    let json = render::render_text("json", EXAMPLE, false).unwrap();
    assert_eq!(HostSpec::from_json(&json).unwrap(), spec);
    assert_eq!(render::render_text("json", &json, true).unwrap(), json);

    let script = render::render_text("prepare", &json, true).unwrap();
    assert!(
        text("units.prepare", 0).contains(&format!("ExecStart=/bin/sh -c '{}'", script.trim_end()))
    );
    assert!(script.contains("mkdir -p /sys/fs/cgroup/maxhq-workspaces;"));

    let rules = render::render_text("egress", &json, true).unwrap();
    assert_eq!(text("egress", 1), format!("{MARKER}\n{rules}"));
    assert!(rules.starts_with("table inet maxhq-egress {}"));

    let low = render::render_text("memory-low", &json, true).unwrap();
    assert_eq!(low, format!("[Slice]\nMemoryLow={}\n", 4u64 << 30));

    // A file on disk reads by its name.
    host.write("spec/host.json", &json);
    assert_eq!(
        render("egress", &host.path("spec/host.json")).unwrap(),
        rules
    );
}

#[test]
fn render_refuses_a_bad_spec_and_what_it_lacks() {
    let err = render::render_text("json", "[caps]\ncpu_weight = 0", false).unwrap_err();
    assert!(err.contains("caps.cpu_weight"), "{err}");
    for (item, words) in [
        ("prepare", "caps.manage_parent"),
        ("egress", "incus.bridge"),
        ("memory-low", "caps.system_memory_low"),
        ("units", "only json, prepare"),
    ] {
        let err = render::render_text(item, "{}", true).unwrap_err();
        assert!(err.contains(words), "{item}: {err}");
    }
    // Incus off means no egress table, even with a bridge.
    let off = r#"{"incus":{"enable":false,"bridge":{"name":"agentbr0"}}}"#;
    assert!(render::render_text("egress", off, true).is_err());
}

#[tokio::test]
async fn macos_checks_only_what_applies() {
    let host = Host::new("");
    let mut env = host.env(None, &["git", "claude"]);
    env.os = Os::Mac;
    let spec = HostSpec::from_toml(EXAMPLE).unwrap();
    let items = check::items(&env, &spec).await;
    assert_eq!(
        ids(&items),
        [
            "service.user",
            "service",
            "tools.claude",
            "tools.codex",
            "tools.cursor",
            "tools.node",
            "tools.system.git",
        ]
    );
    assert!(items.iter().all(|item| !matches!(
        item.fix,
        Some(HostFix::RootCommand { .. } | HostFix::NixChange { .. })
    )));
}

#[test]
fn system_refuses_off_debian_and_ubuntu() {
    let spec = HostSpec::default();
    for (release, words) in [
        ("ID=nixos\n", NIXOS.to_owned()),
        ("ID=fedora\n", "this host runs fedora".to_owned()),
    ] {
        let host = Host::new(release);
        let env = host.env(None, &[]);
        let mut exec = FakeExec::new(host.root());
        let err = system::plan(&env, &spec, &mut exec).unwrap_err();
        assert!(err.contains(&words), "{err}");
        assert!(exec.calls.is_empty());
    }
    assert_eq!(
        NIXOS,
        "This host is NixOS. Change maxhq-host.toml and rebuild."
    );
}

#[test]
fn system_refuses_another_service_user() {
    let host = Host::new(NOBLE);
    let mut env = host.env(None, &[]);
    env.user.name = "other".into();
    let spec = HostSpec::from_toml(EXAMPLE).unwrap();
    let err = system::plan(&env, &spec, &mut FakeExec::new(host.root())).unwrap_err();
    assert!(err.contains("as dev"), "{err}");
}

#[test]
fn the_units_say_what_they_do() {
    let host = Host::new(NOBLE);
    let env = host.env(None, &[]);
    let spec = HostSpec::from_toml(EXAMPLE).unwrap();
    let wants = system::files(&env, &spec);
    let text = |id: &str, n: usize| {
        wants.iter().find(|want| want.id == id).unwrap().files[n]
            .1
            .clone()
    };
    let prepare = text("units.prepare", 0);
    assert!(prepare.starts_with(MARKER));
    assert!(prepare.contains("Before=incus.service"));
    assert!(prepare.contains("mkdir -p /sys/fs/cgroup/maxhq-workspaces;"));
    assert!(prepare.contains("mkdir -p /sys/fs/cgroup/maxhq-workspaces.monitor;"));
    assert!(prepare.contains("echo 800000 100000 > /sys/fs/cgroup/maxhq-workspaces/cpu.max"));
    assert!(prepare.contains(&format!("echo {} > ", 12u64 << 30)));
    assert!(!prepare.contains("memory.max"));

    let shutdown = text("units.shutdown", 0);
    assert!(shutdown.contains("User=dev"));
    assert!(shutdown.contains(&format!(
        "After=incus.service user@{}.service",
        env.user.uid
    )));
    // As root, since maxd winds agents down unproven only for root.
    assert!(shutdown.contains("ExecStop=+/home/dev/.local/bin/maxd host quiesce --window 1200s"));
    assert!(shutdown.contains("TimeoutStopSec=1260"));

    let rules = text("egress", 1);
    for range in crate::workspace::egress::BLOCKED {
        assert!(rules.contains(range), "{range}");
    }
    assert!(rules.contains("198.18.0.0/15"));
    assert!(rules.contains("iifname \"agentbr0\" drop"));
    assert!(rules.contains("delete table inet maxhq-egress"));

    // Without a parent to manage, there's no prepare unit.
    let mut spec = spec;
    spec.caps.manage_parent = false;
    let ids: Vec<_> = system::files(&env, &spec)
        .iter()
        .map(|want| want.id)
        .collect();
    assert_eq!(ids, ["units.shutdown", "egress"]);
}

#[tokio::test]
async fn system_prints_its_plan_runs_it_and_then_has_nothing_to_do() {
    let host = Host::new(BOOKWORM);
    host.spec(EXAMPLE);
    let env = host.env(None, &[]);
    let mut exec = FakeExec::new(host.root());
    let mut out = Vec::new();
    let mut asked = false;
    let mut confirm = || {
        asked = true;
        true
    };
    system::apply(
        &env,
        &mut exec,
        system::Ask::Terminal(&mut confirm),
        &mut out,
    )
    .unwrap();
    assert!(asked);
    let out = String::from_utf8(out).unwrap();
    let plan = out.split("maxd will:").nth(1).unwrap();
    // The plan names every change before any runs.
    for change in exec.changes() {
        assert!(plan.contains(change), "{change} isn't in the plan:\n{out}");
    }
    let changes = exec.changes().join("\n");
    // Bookworm's archive has no Incus, so Zabbly's comes first.
    assert!(changes.contains("curl -fsSL -o /etc/apt/keyrings/zabbly.asc.download"));
    assert!(plan.contains(&format!(
        "Check the key's fingerprint is {}, then put it at /etc/apt/keyrings/zabbly.asc",
        system::ZABBLY_FINGERPRINT
    )));
    assert!(
        changes.contains("apt-get install -y --no-install-recommends curl ca-certificates gnupg")
    );
    assert_eq!(host.read("etc/apt/keyrings/zabbly.asc"), "key");
    assert!(!host.path("etc/apt/keyrings/zabbly.asc.download").exists());
    assert!(host
        .read("etc/apt/sources.list.d/zabbly-incus-stable.sources")
        .contains("Suites: bookworm"));
    assert!(changes.contains(
        "env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends git incus nftables systemd-zram-generator"
    ));
    assert!(changes.contains("usermod -aG incus-admin dev"));
    assert!(changes.contains("loginctl enable-linger dev"));
    assert!(changes.contains("systemctl enable --now maxd-shutdown.service"));
    assert!(changes.contains("systemctl enable --now maxd-egress-check.timer"));
    assert!(changes.contains(&format!(
        "systemctl set-property --runtime system.slice MemoryLow={}",
        4u64 << 30
    )));
    for file in [
        "etc/systemd/system/maxd-prepare.service",
        "etc/systemd/system/maxd-shutdown.service",
        "etc/systemd/system/maxd-egress.service",
        "etc/maxhq/egress.nft",
        "etc/systemd/zram-generator.conf",
        "etc/systemd/system/system.slice.d/50-maxhq-memory-low.conf",
    ] {
        assert!(host.read(file).starts_with(MARKER), "{file}");
    }
    assert!(host.path("srv/maxhq").is_dir());
    assert_eq!(
        fs::metadata(host.path("srv/maxhq"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    // The check agrees.
    let items = check::items(&env, &HostSpec::from_toml(EXAMPLE).unwrap()).await;
    for id in [
        "incus.group",
        "linger",
        "zram",
        "caps.system_memory_low",
        "units.prepare",
        "units.shutdown",
        "egress",
        "data_dir",
    ] {
        assert_eq!(item(&items, id).state, HostItemState::Ok, "{id}");
    }

    // A second run changes nothing.
    let before = exec.calls.len();
    let mut out = Vec::new();
    let mut confirm = || panic!("nothing to confirm");
    system::apply(
        &env,
        &mut exec,
        system::Ask::Terminal(&mut confirm),
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out)
        .unwrap()
        .contains("Nothing to change."));
    let again: Vec<_> = exec.calls[before..]
        .iter()
        .filter(|call| {
            !call.starts_with("dpkg-query")
                && !call.starts_with("systemctl is-")
                && !call.starts_with("gpg --show-keys")
        })
        .collect();
    assert!(again.is_empty(), "{again:?}");
}

#[tokio::test]
async fn a_data_folder_others_can_read_reads_as_drift_and_gets_tightened() {
    let host = Host::new(BOOKWORM);
    host.spec(EXAMPLE);
    let env = host.env(None, &[]);
    fs::create_dir_all(host.path("srv/maxhq")).unwrap();
    fs::set_permissions(host.path("srv/maxhq"), fs::Permissions::from_mode(0o755)).unwrap();
    let items = check::items(&env, &HostSpec::from_toml(EXAMPLE).unwrap()).await;
    assert_eq!(
        item(&items, "data_dir").state,
        HostItemState::Drift {
            want: "mode 0700".to_owned(),
            have: "mode 0755".to_owned(),
        }
    );
    let mut exec = FakeExec::new(host.root());
    let mut out = Vec::new();
    let mut confirm = || true;
    system::apply(
        &env,
        &mut exec,
        system::Ask::Terminal(&mut confirm),
        &mut out,
    )
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(exec.changes().contains(&"chmod 0700 /srv/maxhq"), "{out}");
    assert!(out.contains("chmod 0700 /srv/maxhq"), "{out}");
}

#[test]
fn a_changed_spec_restarts_only_what_may_restart() {
    let host = Host::new(NOBLE);
    let env = host.env(None, &[]);
    let spec = HostSpec::from_toml(EXAMPLE).unwrap();
    let mut exec = FakeExec::new(host.root());
    for want in system::files(&env, &spec) {
        for (path, _) in &want.files {
            host.write(path, &format!("{MARKER}\nolder\n"));
        }
    }
    for unit in [
        "maxd-prepare.service",
        "maxd-shutdown.service",
        "maxd-egress.service",
        "maxd-egress-check.timer",
    ] {
        exec.up.insert(unit.to_owned());
    }
    let plan = system::plan(&env, &spec, &mut exec).unwrap();
    let lines: Vec<String> = plan.steps.iter().map(Step::line).collect();
    assert!(lines.contains(&"Run systemctl restart maxd-prepare.service".to_owned()));
    assert!(lines.contains(&"Run systemctl restart maxd-egress.service".to_owned()));
    assert!(!lines
        .iter()
        .any(|line| line.contains("maxd-shutdown.service") && !line.starts_with("Write")));
}

#[test]
fn a_file_maxd_didnt_write_stops_everything() {
    let host = Host::new(NOBLE);
    host.spec(EXAMPLE);
    host.write(
        "etc/systemd/zram-generator.conf",
        "[zram0]\nzram-size = ram\n",
    );
    let env = host.env(None, &[]);
    let mut exec = FakeExec::new(host.root());
    let mut out = Vec::new();
    let err = system::apply(&env, &mut exec, system::Ask::Yes, &mut out).unwrap_err();
    assert!(err.contains("Fix what's above"), "{err}");
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("zram-generator.conf is there already"),
        "{out}"
    );
    assert!(exec.changes().is_empty(), "{:?}", exec.changes());
    assert!(!host
        .path("etc/systemd/system/maxd-shutdown.service")
        .exists());
    assert_eq!(
        host.read("etc/systemd/zram-generator.conf"),
        "[zram0]\nzram-size = ram\n"
    );
}

#[test]
fn no_means_nothing_changes_and_a_failure_stops_there() {
    let host = Host::new(NOBLE);
    host.spec(EXAMPLE);
    let env = host.env(None, &[]);
    let mut exec = FakeExec::new(host.root());
    let err = system::apply(
        &env,
        &mut exec,
        system::Ask::Terminal(&mut || false),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.contains("Nothing changed"), "{err}");
    assert!(exec.changes().is_empty());

    exec.fails = Some("usermod".into());
    let err = system::apply(&env, &mut exec, system::Ask::Yes, &mut Vec::new()).unwrap_err();
    assert!(err.contains("usermod -aG incus-admin dev failed"), "{err}");
    assert!(!exec.calls.iter().any(|call| call.starts_with("loginctl")));
    assert!(!host
        .path("etc/systemd/system/maxd-shutdown.service")
        .exists());
}

#[test]
fn the_spec_file_lives_by_the_app_on_a_mac() {
    let home = Path::new("/home/dev");
    assert_eq!(
        spec_path(&Os::Mac, home),
        home.join("Library/Application Support/MaxHQ")
            .join(host_spec::FILE)
    );
    assert_eq!(
        spec_path(&Os::Nix, home),
        home.join(".config/maxhq/maxhq-host.toml")
    );
}

#[test]
fn lines_say_the_fix() {
    let line = super::line(&HostItem {
        id: "zram".into(),
        label: "Compressed swap in memory".into(),
        state: HostItemState::Missing,
        fix: root_fix(),
    });
    assert_eq!(
        line,
        "Compressed swap in memory: missing\n    fix: sudo /home/dev/.local/bin/maxd host apply --system"
    );
}

#[test]
fn a_key_with_another_fingerprint_is_never_trusted() {
    let host = Host::new(BOOKWORM);
    host.spec(EXAMPLE);
    let env = host.env(None, &[]);
    let mut exec = FakeExec::new(host.root());
    exec.gpg = ZABBLY_COLONS.replace("4EFC5906", "0000AAAA");
    let err = system::apply(&env, &mut exec, system::Ask::Yes, &mut Vec::new()).unwrap_err();
    assert!(err.contains("isn't Zabbly's"), "{err}");
    assert!(err.contains(system::ZABBLY_FINGERPRINT), "{err}");
    assert!(!host.path("etc/apt/keyrings/zabbly.asc").exists());
    assert!(!host.path("etc/apt/keyrings/zabbly.asc.download").exists());
    assert!(!host
        .path("etc/apt/sources.list.d/zabbly-incus-stable.sources")
        .exists());
    assert!(!exec
        .calls
        .iter()
        .any(|call| call.contains("install -y --no-install-recommends git")));

    // Nor is a file gpg can't read.
    exec.gpg = String::new();
    let err = system::apply(&env, &mut exec, system::Ask::Yes, &mut Vec::new()).unwrap_err();
    assert!(err.contains("no key gpg could read"), "{err}");
    assert!(!host.path("etc/apt/keyrings/zabbly.asc").exists());
}

#[test]
fn without_a_terminal_only_yes_goes_ahead() {
    let host = Host::new(NOBLE);
    host.spec(EXAMPLE);
    let env = host.env(None, &[]);
    let mut exec = FakeExec::new(host.root());
    let mut out = Vec::new();
    let err = system::apply(&env, &mut exec, system::Ask::NoTerminal, &mut out).unwrap_err();
    assert_eq!(err, system::NO_TERMINAL);
    assert_eq!(
        err,
        "No terminal to ask on. Run it again with --yes to go ahead."
    );
    // It still shows the plan, and changes nothing.
    assert!(String::from_utf8(out).unwrap().contains("maxd will:"));
    assert!(exec.changes().is_empty(), "{:?}", exec.changes());

    let mut out = Vec::new();
    system::apply(&env, &mut exec, system::Ask::Yes, &mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("Done."));
    assert!(host
        .read("etc/systemd/system/maxd-shutdown.service")
        .starts_with(MARKER));
}
