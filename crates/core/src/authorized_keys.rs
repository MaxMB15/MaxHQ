// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's lines in `authorized_keys`: the app's key, which may run only
//! `maxd proxy`, and each other device's, which may run only `maxd proxy
//! --peer` for that device. Every line MaxHQ writes ends in its own tag,
//! and nothing here touches a line without one.
//!
//! A change is written to a new file beside the old one and renamed over
//! it, with the old file's mode, so a crash leaves one whole file.

use std::fs;
use std::io;
use std::path::Path;

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

/// The line that lets a phone's `public_key` run only `maxd proxy --phone
/// <phone>`, for a line you add by hand. The command starts at `~`, which
/// the device's shell reads as your home folder.
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
    Ok(format!(
        "restrict,command=\"~/.local/bin/maxd proxy {flag} {name}\" {kind} {key} {tag}"
    ))
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
