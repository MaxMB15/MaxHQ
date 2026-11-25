// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This device's keys on macOS and Linux: in the home folder, with lines
//! that run `maxd` through the stable symlink there.

use std::path::{Path, PathBuf};

/// The `authorized_keys` this device's sshd reads for this account:
/// `.ssh/authorized_keys` in `home`.
pub fn file_here(home: &Path) -> PathBuf {
    super::in_home(home)
}

/// The line [`line`](super::line) gives for `home`.
pub fn line_here(home: &str, public_key: &str, peer: Option<&str>) -> Result<String, String> {
    super::line(home, public_key, peer)
}

/// `maxd proxy` with `flags` for a line added by hand, from `~`, which the
/// device's shell reads as the home folder.
pub(super) fn forced_command(flags: &str) -> Result<String, String> {
    Ok(format!("~/.local/bin/maxd proxy {flags}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorized_keys::phone_line;
    use crate::authorized_keys::tests::KEY;

    #[test]
    fn the_file_here_is_in_the_home_folder() {
        assert_eq!(
            file_here(Path::new("/home/dev")),
            Path::new("/home/dev/.ssh/authorized_keys")
        );
    }

    #[test]
    fn a_line_here_is_the_unix_one() {
        assert_eq!(
            line_here("/home/dev", KEY, None),
            super::super::line("/home/dev", KEY, None)
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
}
