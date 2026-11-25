// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What this computer is called, in the label a device shows beside its
//! app key: "MaxHQ on <name>".

/// This computer's name, the same one maxd gives the device, so the label
/// and the device card agree.
pub(super) fn computer_name() -> String {
    maxhq_os::computer::name().unwrap_or_else(|| FALLBACK.to_owned())
}

/// What the label says when the system gives no name.
#[cfg(target_os = "macos")]
const FALLBACK: &str = "this Mac";
#[cfg(windows)]
const FALLBACK: &str = "this PC";
#[cfg(not(any(target_os = "macos", windows)))]
const FALLBACK: &str = "this computer";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_computer_has_a_label_name() {
        assert!(!computer_name().is_empty());
    }
}
