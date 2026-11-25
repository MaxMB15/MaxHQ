// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's lines in `authorized_keys`: the app's key, which may run only
//! `maxd proxy`, and each other device's, which may run only `maxd proxy
//! --peer` for that device. Every line MaxHQ writes ends in its own tag,
//! and nothing here touches a line without one.
//!
//! A change is written to a new file beside the old one and renamed over
//! it, with the old file's mode, so a crash leaves one whole file.
//!
//! Windows OpenSSH runs a key's command through the account's shell,
//! `cmd.exe /c` unless `DefaultShell` names another, where `~` means
//! nothing. Its lines name `maxd.exe` by its full path, in quotes when the
//! path has a space, and an Administrators member's keys are in
//! [`windows_admin_file`] rather than their own folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use maxhq_os::fsmode;

use crate::fs::Atomic;

/// The tag on the app's key line.
pub const APP_TAG: &str = "maxhq";

/// The tag on another device's key line.
pub fn peer_tag(device: &str) -> String {
    format!("maxhq-peer-{device}")
}

/// The tag on a phone's key line.
pub fn phone_tag(phone: &str) -> String {
    format!("maxhq-phone-{phone}")
}

/// The prefix of the tag on a one-time pairing key's line.
pub const PAIR_TAG: &str = "maxhq-pair-";

/// The tag on a one-time pairing key's line.
pub fn pair_tag(pair: &str) -> String {
    format!("{PAIR_TAG}{pair}")
}

/// Where `maxd.exe` stays under `%LOCALAPPDATA%` as updates come: the
/// folder an update points at the build it installs.
pub const WINDOWS_MAXD: &str = r"MaxHQ\bin\current\maxd.exe";

/// Where Windows OpenSSH reads the keys of an Administrators member, under
/// `%ProgramData%`. Only Administrators and SYSTEM may write it.
pub const WINDOWS_ADMIN_KEYS: &str = r"ssh\administrators_authorized_keys";

/// [`WINDOWS_ADMIN_KEYS`] under `program_data`.
pub fn windows_admin_file(program_data: &Path) -> PathBuf {
    program_data.join(WINDOWS_ADMIN_KEYS)
}

/// The `authorized_keys` this device's sshd reads for this account:
/// `.ssh\authorized_keys` in `home`, or on Windows [`windows_admin_file`]
/// for an Administrators member when `sshd_config` sends their keys there,
/// as it does from install.
pub fn file_here(home: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(file) = windows_admin_file_here() {
        return file;
    }
    home.join(".ssh").join("authorized_keys")
}

/// [`windows_admin_file`] when sshd reads this account's keys from it.
#[cfg(windows)]
fn windows_admin_file_here() -> Option<PathBuf> {
    let program_data = PathBuf::from(std::env::var_os("ProgramData")?);
    let config = fs::read_to_string(program_data.join("ssh").join("sshd_config")).ok();
    (windows_admin_keys_apply(config.as_deref()) && windows_admin())
        .then(|| windows_admin_file(&program_data))
}

/// Whether this account is in Administrators, from `whoami /groups`.
#[cfg(windows)]
fn windows_admin() -> bool {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    std::process::Command::new(root.join("System32").join("whoami.exe"))
        .args(["/groups", "/fo", "csv", "/nh"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|out| {
            out.status.success() && in_administrators(&String::from_utf8_lossy(&out.stdout))
        })
}

/// Whether `whoami /groups /fo csv /nh` lists Administrators, `S-1-5-32-544`,
/// enabled or for deny only. sshd's `Match Group` reads who's a member, not
/// whether the process runs elevated.
pub fn in_administrators(groups: &str) -> bool {
    groups.contains("\"S-1-5-32-544\"")
}

/// Whether a Windows `sshd_config` reads Administrators' keys from
/// [`WINDOWS_ADMIN_KEYS`]: a `Match Group` for `administrators` with an
/// `AuthorizedKeysFile` naming it. A missing file counts, since sshd
/// writes its default, which has that block, when it first starts.
pub fn windows_admin_keys_apply(sshd_config: Option<&str>) -> bool {
    let Some(config) = sshd_config else {
        return true;
    };
    let mut admins = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let mut words = line
            .split(|c: char| c.is_whitespace() || c == '=')
            .filter(|word| !word.is_empty());
        let Some(keyword) = words.next() else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("match") {
            let criteria: Vec<&str> = words.collect();
            admins = criteria.windows(2).any(|pair| {
                pair[0].eq_ignore_ascii_case("group")
                    && pair[1]
                        .split(',')
                        .any(|group| group.eq_ignore_ascii_case("administrators"))
            });
        } else if admins && keyword.eq_ignore_ascii_case("authorizedkeysfile") {
            let named = words.any(|file| {
                file.to_ascii_lowercase()
                    .trim_matches('"')
                    .ends_with("administrators_authorized_keys")
            });
            if named {
                return true;
            }
        }
    }
    false
}

/// What to say when `line` couldn't go in `file`. Only an elevated process
/// may write [`windows_admin_file`], so for it this gives the PowerShell
/// that adds the line, to run as administrator.
pub fn add_failed(file: &Path, line: &str, err: &io::Error) -> String {
    let admin = file
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with("administrators_authorized_keys");
    if !admin || err.kind() != io::ErrorKind::PermissionDenied {
        return format!("Couldn't write {}: {err}", file.display());
    }
    let quoted = crate::shell::powershell::literal;
    let path = quoted(&file.to_string_lossy());
    format!(
        "Windows OpenSSH reads an administrator's keys from {file}, which only an elevated \
         process can change. Open PowerShell as administrator and run:\n\
         Add-Content -Force -Path {path} -Value {value}\n\
         icacls.exe {path} /inheritance:r /grant 'Administrators:F' /grant 'SYSTEM:F'",
        file = file.display(),
        value = quoted(line),
    )
}

/// The stable `maxd.exe` on this Windows machine, or an error naming what
/// it lacks.
fn windows_maxd() -> Result<String, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or("LOCALAPPDATA isn't set, so the key line can't name maxd.exe")?;
    let base = base.to_string_lossy();
    Ok(format!(
        "{}\\{WINDOWS_MAXD}",
        base.trim_end_matches(['\\', '/'])
    ))
}

/// The line [`line`] gives on Unix and [`windows_line`] on Windows, where
/// `maxd.exe` is [`WINDOWS_MAXD`] under `%LOCALAPPDATA%` and `home` isn't
/// read.
pub fn line_here(home: &str, public_key: &str, peer: Option<&str>) -> Result<String, String> {
    if cfg!(windows) {
        windows_line(&windows_maxd()?, public_key, peer)
    } else {
        line(home, public_key, peer)
    }
}

/// The line that lets `public_key` run only `maxd proxy` through the
/// stable symlink in `home`, or `maxd proxy --peer <device>` for another
/// device's key.
pub fn line(home: &str, public_key: &str, peer: Option<&str>) -> Result<String, String> {
    let (kind, key) = key_parts(public_key, peer)?;
    if !home.starts_with('/') || home.contains(['"', '\'', '\\', '\n', ' ']) {
        return Err(format!(
            "The device's home folder {home} can't go in a key line"
        ));
    }
    Ok(match peer {
        None => format!("restrict,command=\"{home}/.local/bin/maxd proxy\" {kind} {key} {APP_TAG}"),
        Some(device) => format!(
            "restrict,command=\"{home}/.local/bin/maxd proxy --peer {device}\" {kind} {key} {}",
            peer_tag(device)
        ),
    })
}

/// The line that lets `public_key` run only `"<maxd>" proxy`, or
/// `"<maxd>" proxy --peer <device>` for another device's key, on Windows.
/// `maxd` is the full path of `maxd.exe`, from a drive letter.
///
/// The path is quoted only when it has a space. Windows OpenSSH runs the
/// command as `cmd.exe /c "<command>"`, and cmd keeps a quoted program
/// with a space whole whether or not those outer quotes are there. A
/// character cmd or PowerShell would read, such as `%`, `^`, `&` or `'`,
/// is refused rather than escaped.
pub fn windows_line(maxd: &str, public_key: &str, peer: Option<&str>) -> Result<String, String> {
    let (kind, key) = key_parts(public_key, peer)?;
    let command = windows_command(maxd, &peer.map(|device| format!("--peer {device}")))?;
    let tag = match peer {
        None => APP_TAG.to_owned(),
        Some(device) => peer_tag(device),
    };
    Ok(format!("restrict,command=\"{command}\" {kind} {key} {tag}"))
}

/// `maxd proxy` with `flags` as a Windows key line's command, with the
/// quotes around a program with a space escaped for `authorized_keys`.
fn windows_command(maxd: &str, flags: &Option<String>) -> Result<String, String> {
    let bytes = maxd.as_bytes();
    let from_a_drive =
        bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let odd = maxd
        .chars()
        .any(|c| c.is_control() || "\"%^&|<>!'`$()/;,=".contains(c));
    if !from_a_drive || odd || maxd.contains("  ") || maxd.ends_with([' ', '\\']) {
        return Err(format!("The path {maxd} can't go in a key line"));
    }
    let program = match maxd.contains(' ') {
        true => format!("\\\"{maxd}\\\""),
        false => maxd.to_owned(),
    };
    Ok(match flags {
        Some(flags) => format!("{program} proxy {flags}"),
        None => format!("{program} proxy"),
    })
}

/// The line that lets a phone's `public_key` run only `maxd proxy --phone
/// <phone>`, for a line you add by hand. The command starts at `~`, which
/// the device's shell reads as your home folder. On Windows it names
/// [`WINDOWS_MAXD`] under `%LOCALAPPDATA%`.
pub fn phone_line(public_key: &str, phone: &str) -> Result<String, String> {
    home_line(public_key, "--phone", phone, &phone_tag(phone))
}

/// The line that lets a one-time pairing key run only `maxd proxy --pair
/// <pair>`, the same way.
pub fn pair_line(public_key: &str, pair: &str) -> Result<String, String> {
    home_line(public_key, "--pair", pair, &pair_tag(pair))
}

fn home_line(public_key: &str, flag: &str, name: &str, tag: &str) -> Result<String, String> {
    let (kind, key) = key_parts(public_key, Some(name))?;
    let command = match cfg!(windows) {
        true => windows_command(&windows_maxd()?, &Some(format!("{flag} {name}")))?,
        false => format!("~/.local/bin/maxd proxy {flag} {name}"),
    };
    Ok(format!("restrict,command=\"{command}\" {kind} {key} {tag}"))
}

/// A public key's type and key, checked to be plain, with the name a
/// line's command takes.
fn key_parts<'a>(public_key: &'a str, name: Option<&str>) -> Result<(&'a str, &'a str), String> {
    let mut parts = public_key.split_whitespace();
    let (Some(kind), Some(key)) = (parts.next(), parts.next()) else {
        return Err("The public key isn't in the form ssh writes".into());
    };
    let plain = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+/=-_.@".contains(c))
    };
    if !plain(kind) || !plain(key) || name.is_some_and(|name| !plain(name)) {
        return Err("The public key isn't in the form ssh writes".into());
    }
    Ok((kind, key))
}

/// Adds `line` unless the file has it already, and says whether it did.
/// A file that isn't there is made, 0600.
pub fn add(path: &Path, line: &str) -> io::Result<bool> {
    let text = read(path)?;
    if text.lines().any(|each| each.trim() == line) {
        return Ok(false);
    }
    let mut next = text;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(line);
    next.push('\n');
    replace(path, &next)?;
    Ok(true)
}

/// Takes out every line that ends in `tag` and runs a MaxHQ command, and
/// says whether any went.
pub fn remove(path: &Path, tag: &str) -> io::Result<bool> {
    remove_where(path, |each| each == tag)
}

/// Takes out every line that runs a MaxHQ command and whose tag passes
/// `tag`, and says whether any went.
pub fn remove_where(path: &Path, tag: impl Fn(&str) -> bool) -> io::Result<bool> {
    let text = read(path)?;
    let ours = |line: &str| {
        let line = line.trim();
        line.starts_with("restrict,command=\"") && line.split_whitespace().last().is_some_and(&tag)
    };
    if !text.lines().any(ours) {
        return Ok(false);
    }
    let mut next = String::with_capacity(text.len());
    for line in text.lines().filter(|line| !ours(line)) {
        next.push_str(line);
        next.push('\n');
    }
    replace(path, &next)?;
    Ok(true)
}

/// The keys on the lines that run a MaxHQ command and end in `tag`, each
/// as `<type> <base64>`, in the file's order. A file that isn't there has
/// none.
pub fn keys_tagged(path: &Path, tag: &str) -> io::Result<Vec<String>> {
    let text = read(path)?;
    let key = |line: &str| {
        let line = line.trim();
        if !line.starts_with("restrict,command=\"") {
            return None;
        }
        let mut words = line.split_whitespace().rev();
        match (words.next(), words.next(), words.next()) {
            (Some(last), Some(key), Some(kind)) if last == tag => Some(format!("{kind} {key}")),
            _ => None,
        }
    };
    Ok(text.lines().filter_map(key).collect())
}

/// The file's text, or none when it isn't there.
fn read(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

/// Writes `text` to a new file with the old one's mode, or 0600, and
/// renames it over `path`. The folder is made 0700 if it's missing.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let mode = match fs::metadata(path) {
        Ok(meta) => fsmode::mode_of(&meta) & 0o777,
        Err(err) if err.kind() == io::ErrorKind::NotFound => 0o600,
        Err(err) => return Err(err),
    };
    if let Some(dir) = path.parent() {
        fsmode::private_dir_all(dir)?;
    }
    crate::fs::write_atomic(path, text.as_bytes(), Atomic::exact(mode))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample";

    struct TempHome(PathBuf);

    impl TempHome {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-keys-{}", uuid::Uuid::now_v7()));
            fs::create_dir_all(&dir).expect("temp home");
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(".ssh").join("authorized_keys")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        fsmode::mode_of(&fs::metadata(path).expect("meta")) & 0o777
    }

    #[test]
    fn the_app_line_runs_only_maxd_proxy() {
        assert_eq!(
            line("/home/dev", "ssh-ed25519 AAAAkey maxhq@mac", None).as_deref(),
            Ok("restrict,command=\"/home/dev/.local/bin/maxd proxy\" ssh-ed25519 AAAAkey maxhq")
        );
    }

    #[test]
    fn a_peer_line_runs_only_maxd_proxy_for_that_device() {
        assert_eq!(
            line("/home/dev", KEY, Some("d1")).as_deref(),
            Ok(format!(
                "restrict,command=\"/home/dev/.local/bin/maxd proxy --peer d1\" {KEY} maxhq-peer-d1"
            )
            .as_str())
        );
    }

    #[test]
    fn a_windows_line_names_maxd_exe_by_its_full_path() {
        let maxd = r"C:\Users\dev\AppData\Local\MaxHQ\bin\current\maxd.exe";
        assert_eq!(
            windows_line(maxd, KEY, None).as_deref(),
            Ok(format!("restrict,command=\"{maxd} proxy\" {KEY} maxhq").as_str())
        );
        assert_eq!(
            windows_line(maxd, KEY, Some("d1")).as_deref(),
            Ok(format!("restrict,command=\"{maxd} proxy --peer d1\" {KEY} maxhq-peer-d1").as_str())
        );
    }

    #[test]
    fn a_windows_path_with_a_space_is_quoted_for_cmd() {
        let maxd = r"C:\Users\Dev One\AppData\Local\MaxHQ\bin\current\maxd.exe";
        // authorized_keys reads `\"` as a quote, so cmd sees
        // `"C:\Users\Dev One\...\maxd.exe" proxy`.
        assert_eq!(
            windows_line(maxd, KEY, Some("d1")).as_deref(),
            Ok(
                format!(r#"restrict,command="\"{maxd}\" proxy --peer d1" {KEY} maxhq-peer-d1"#)
                    .as_str()
            )
        );
        assert_eq!(
            windows_command(maxd, &Some("--phone p1".into())).as_deref(),
            Ok(format!(r#"\"{maxd}\" proxy --phone p1"#).as_str())
        );
    }

    #[test]
    fn windows_paths_cmd_or_powershell_would_read_are_refused() {
        for maxd in [
            r"maxd.exe",
            r"\\server\share\maxd.exe",
            r"C:maxd.exe",
            r"C:/Users/dev/maxd.exe",
            r"C:\Users\100%\maxd.exe",
            r"C:\Users\a^b\maxd.exe",
            r"C:\Users\a&b\maxd.exe",
            r"C:\Users\a|b\maxd.exe",
            r"C:\Users\a>b\maxd.exe",
            r"C:\Users\a!b\maxd.exe",
            r#"C:\Users\a"b\maxd.exe"#,
            r"C:\Users\o'neil\maxd.exe",
            r"C:\Users\$env\maxd.exe",
            r"C:\Users\a`b\maxd.exe",
            r"C:\Program Files (x86)\maxd.exe",
            "C:\\Users\\a\nb\\maxd.exe",
            r"C:\Users\a  b\maxd.exe",
            r"C:\Users\dev\ ",
        ] {
            assert!(windows_line(maxd, KEY, None).is_err(), "{maxd}");
        }
        let maxd = r"C:\Users\dev\maxd.exe";
        assert!(windows_line(maxd, KEY, Some("d1 x")).is_err());
        assert!(windows_line(maxd, "ssh-ed25519", None).is_err());
    }

    #[test]
    fn a_windows_admins_keys_are_in_program_data() {
        assert_eq!(
            windows_admin_file(Path::new("/ProgramData")),
            Path::new("/ProgramData").join(r"ssh\administrators_authorized_keys")
        );
    }

    #[test]
    fn administrators_are_read_from_whoami_by_their_sid() {
        let groups = "\"Everyone\",\"Well-known group\",\"S-1-1-0\",\"Mandatory group\"\r\n\
             \"BUILTIN\\Administrators\",\"Alias\",\"S-1-5-32-544\",\"Group used for deny only\"\r\n";
        assert!(in_administrators(groups));
        let user = "\"BUILTIN\\Users\",\"Alias\",\"S-1-5-32-545\",\"Mandatory group\"\r\n";
        assert!(!in_administrators(user));
        assert!(!in_administrators("\"S-1-5-32-5440\""));
    }

    #[test]
    fn the_admin_file_counts_only_while_sshd_config_names_it() {
        let shipped = "# This is the sshd server system-wide configuration file.\n\
             AuthorizedKeysFile\t.ssh/authorized_keys\n\
             Subsystem\tsftp\tsftp-server.exe\n\
             Match Group administrators\n\
             \x20      AuthorizedKeysFile __PROGRAMDATA__/ssh/administrators_authorized_keys\n";
        assert!(windows_admin_keys_apply(Some(shipped)));
        assert!(windows_admin_keys_apply(None));
        let commented = shipped.replace("Match Group", "#Match Group").replace(
            "       AuthorizedKeysFile __",
            "#       AuthorizedKeysFile __",
        );
        assert!(!windows_admin_keys_apply(Some(&commented)));
        let other_group = shipped.replace("Group administrators", "Group devs");
        assert!(!windows_admin_keys_apply(Some(&other_group)));
        let listed = "Match User x Group devs,Administrators\n\
             AuthorizedKeysFile=\"__PROGRAMDATA__/ssh/administrators_authorized_keys\"\n";
        assert!(windows_admin_keys_apply(Some(listed)));
        // A block that ends before the file is named doesn't count.
        let ended = "Match Group administrators\nMatch all\n\
             AuthorizedKeysFile __PROGRAMDATA__/ssh/administrators_authorized_keys\n";
        assert!(!windows_admin_keys_apply(Some(ended)));
        assert!(!windows_admin_keys_apply(Some("")));
    }

    #[test]
    fn a_refused_admin_file_gives_the_steps_that_add_the_line() {
        let file = Path::new(r"C:\ProgramData\ssh\administrators_authorized_keys");
        let line = r#"restrict,command="\"C:\Users\A B\maxd.exe\" proxy" ssh-ed25519 AAAA maxhq"#;
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let steps = add_failed(file, line, &denied);
        assert!(steps.contains("as administrator"), "{steps}");
        assert!(
            steps.contains(&format!(
                r"Add-Content -Force -Path 'C:\ProgramData\ssh\administrators_authorized_keys' -Value '{line}'"
            )),
            "{steps}"
        );
        assert!(
            steps.contains(
                r"icacls.exe 'C:\ProgramData\ssh\administrators_authorized_keys' /inheritance:r"
            ),
            "{steps}"
        );
        let quoted = add_failed(
            Path::new(r"C:\Data's\administrators_authorized_keys"),
            line,
            &denied,
        );
        assert!(
            quoted.contains(r"'C:\Data''s\administrators_authorized_keys'"),
            "{quoted}"
        );
        // Any other failure, or file, is said as it is.
        let other = add_failed(file, line, &io::Error::other("disk full"));
        assert!(
            other.starts_with("Couldn't write") && other.contains("disk full"),
            "{other}"
        );
        let home = add_failed(Path::new("/home/dev/.ssh/authorized_keys"), line, &denied);
        assert!(home.starts_with("Couldn't write"), "{home}");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_here_is_in_the_home_folder_off_windows() {
        assert_eq!(
            file_here(Path::new("/home/dev")),
            Path::new("/home/dev/.ssh/authorized_keys")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_phone_line_on_windows_names_maxd_exe_under_local_app_data() {
        let line = phone_line(KEY, "p1").expect("line");
        assert!(line.contains(r"\MaxHQ\bin\current\maxd.exe"), "{line}");
        assert!(line.contains(" proxy --phone p1\" "), "{line}");
        assert!(line.ends_with(" maxhq-phone-p1"), "{line}");
    }

    #[cfg(unix)]
    #[test]
    fn a_line_here_is_the_unix_one_off_windows() {
        assert_eq!(
            line_here("/home/dev", KEY, None),
            line("/home/dev", KEY, None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_phone_line_runs_only_maxd_proxy_for_that_phone() {
        let phone = "0b7e3c1a-4d2f-5e6a-8b9c-0d1e2f3a4b5c";
        assert_eq!(
            phone_line(KEY, phone).as_deref(),
            Ok(format!(
                "restrict,command=\"~/.local/bin/maxd proxy --phone {phone}\" {KEY} maxhq-phone-{phone}"
            )
            .as_str())
        );
        assert!(phone_line(KEY, "p 1").is_err());
        assert!(phone_line("ssh-ed25519", phone).is_err());
    }

    #[test]
    fn odd_keys_homes_and_devices_are_refused() {
        assert!(line("/home/dev", "ssh-ed25519", None).is_err());
        assert!(line("/home/dev", "ssh-ed25519 AA\"AA", None).is_err());
        assert!(line("home/dev", KEY, None).is_err());
        assert!(line("/home/my dev", KEY, None).is_err());
        assert!(line("/home/dev", KEY, Some("d1 x")).is_err());
        assert!(line("/home/dev", KEY, Some("d1\"")).is_err());
    }

    // Windows keeps no mode bits to check.
    #[cfg(unix)]
    #[test]
    fn a_line_goes_in_once_with_the_mode_kept_and_others_untouched() {
        let home = TempHome::new();
        let file = home.file();
        let peer = line("/home/dev", KEY, Some("d1")).expect("line");
        assert!(add(&file, &peer).expect("adds"));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().expect("folder")), 0o700);

        // Someone's own lines, one with no newline at the end.
        fs::write(
            &file,
            format!("ssh-rsa AAAAown me@laptop\n{peer}\nssh-ed25519 AAAAother x"),
        )
        .expect("write");
        fsmode::set_mode(&file, 0o640).expect("chmod");
        assert!(!add(&file, &peer).expect("already there"));
        let app = line("/home/dev", KEY, None).expect("line");
        assert!(add(&file, &app).expect("adds"));
        assert_eq!(mode(&file), 0o640);
        assert_eq!(
            fs::read_to_string(&file).expect("read"),
            format!("ssh-rsa AAAAown me@laptop\n{peer}\nssh-ed25519 AAAAother x\n{app}\n")
        );
    }

    #[test]
    fn removing_takes_out_only_that_tag() {
        let home = TempHome::new();
        let file = home.file();
        let one = line("/home/dev", KEY, Some("d1")).expect("line");
        let two = line("/home/dev", KEY, Some("d2")).expect("line");
        let app = line("/home/dev", KEY, None).expect("line");
        // A line someone wrote that ends in the tag but runs no MaxHQ
        // command stays.
        let own = "ssh-ed25519 AAAAmine maxhq-peer-d1";
        for each in [&one, &two, &app] {
            add(&file, each).expect("adds");
        }
        add(&file, own).expect("adds");
        assert!(remove(&file, &peer_tag("d1")).expect("removes"));
        assert!(!remove(&file, &peer_tag("d1")).expect("gone already"));
        assert_eq!(
            fs::read_to_string(&file).expect("read"),
            format!("{two}\n{app}\n{own}\n")
        );
        assert!(!remove(&home.0.join("missing"), APP_TAG).expect("nothing to remove"));
    }

    #[test]
    fn the_keys_on_a_tag_are_only_those_its_maxhq_lines_hold() {
        let home = TempHome::new();
        let file = home.file();
        assert!(keys_tagged(&file, &peer_tag("d1"))
            .expect("no file")
            .is_empty());
        let other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOther";
        let one = line("/home/dev", KEY, Some("d1")).expect("line");
        let two = line("/home/dev", other, Some("d2")).expect("line");
        let app = line("/home/dev", other, None).expect("line");
        // A line someone wrote that ends in the tag but runs no MaxHQ
        // command doesn't count.
        let own = "ssh-ed25519 AAAAmine maxhq-peer-d1";
        for each in [&one, &two, &app] {
            add(&file, each).expect("adds");
        }
        add(&file, own).expect("adds");
        assert_eq!(keys_tagged(&file, &peer_tag("d1")).expect("read"), [KEY]);
        assert_eq!(keys_tagged(&file, &peer_tag("d2")).expect("read"), [other]);
        assert!(keys_tagged(&file, &peer_tag("d3"))
            .expect("read")
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_pair_line_runs_only_maxd_proxy_for_that_pairing_and_goes_with_the_rest() {
        let home = TempHome::new();
        let file = home.file();
        let pair = "0b7e3c1a-4d2f-5e6a-8b9c-0d1e2f3a4b5c";
        let one = pair_line(KEY, pair).expect("line");
        assert_eq!(
            one,
            format!(
                "restrict,command=\"~/.local/bin/maxd proxy --pair {pair}\" {KEY} maxhq-pair-{pair}"
            )
        );
        let two = pair_line(KEY, "p2").expect("line");
        let phone = phone_line(KEY, "p3").expect("line");
        for each in [&one, &two, &phone] {
            add(&file, each).expect("adds");
        }
        assert!(remove_where(&file, |tag| tag.starts_with(PAIR_TAG)).expect("removes"));
        assert_eq!(
            fs::read_to_string(&file).expect("read"),
            format!("{phone}\n")
        );
    }
}
