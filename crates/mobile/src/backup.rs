// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeps the phone's data folder out of its backups. The folder holds the
//! files the phone downloads from the user's devices and the registry, so a
//! backup to iCloud or a computer would carry copies of them off the phone.
//! The flag goes on the folder, which covers everything in it, and again at
//! every open, since a restore to a new phone can drop it. The media cache
//! sits in the app's caches folder, which no backup takes.

use std::path::Path;

/// Leaves `dir` and all it holds out of the phone's backups. On a system
/// with no such flag it does nothing.
pub fn exclude(dir: &Path) -> Result<(), String> {
    system::exclude(dir)
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod system {
    use std::path::Path;
    use std::ptr::{self, NonNull};

    use objc2_core_foundation::{
        kCFURLIsExcludedFromBackupKey, CFBoolean, CFError, CFRetained, CFType, CFURL,
    };

    #[allow(unsafe_code)]
    pub fn exclude(dir: &Path) -> Result<(), String> {
        let url = CFURL::from_directory_path(dir)
            .ok_or_else(|| format!("{} isn't a path the system reads", dir.display()))?;
        let yes: &CFType = CFBoolean::new(true);
        let mut error: *mut CFError = ptr::null_mut();
        // SAFETY: the key is a constant the system defines, it takes a
        // boolean, and `error` is a place for one error.
        let set = unsafe {
            url.set_resource_property_for_key(kCFURLIsExcludedFromBackupKey, Some(yes), &mut error)
        };
        if set {
            return Ok(());
        }
        // SAFETY: a call that fails and sets `error` hands over a reference to it.
        let error = NonNull::new(error).map(|error| unsafe { CFRetained::from_raw(error) });
        Err(error.map_or_else(
            || "the system didn't say why".to_owned(),
            |error| error.to_string(),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Whether the system leaves `dir` out of backups, read afresh.
        #[allow(unsafe_code)]
        fn excluded(dir: &Path) -> bool {
            let url = CFURL::from_directory_path(dir).unwrap();
            let mut value: *mut CFType = ptr::null_mut();
            // SAFETY: the key is a constant the system defines, and `value`
            // is a place for one value, which comes back retained.
            let read = unsafe {
                url.resource_property_for_key(
                    kCFURLIsExcludedFromBackupKey,
                    (&raw mut value).cast(),
                    ptr::null_mut(),
                )
            };
            assert!(read, "couldn't read the flag");
            // SAFETY: a value that comes back is ours to release.
            NonNull::new(value)
                .map(|value| unsafe { CFRetained::from_raw(value) })
                .and_then(|value| value.downcast_ref::<CFBoolean>().map(CFBoolean::as_bool))
                .unwrap_or(false)
        }

        #[test]
        fn leaves_the_folder_out_of_backups() {
            let dir = std::env::temp_dir().join(format!(
                "maxhq-backup-{}",
                maxhq_protocol::ids::InstanceId::generate()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let before = excluded(&dir);
            let result = exclude(&dir);
            let after = excluded(&dir);
            // Twice does no harm, as every open sets it again.
            let again = exclude(&dir);
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(!before);
            assert_eq!(result, Ok(()));
            assert!(after);
            assert_eq!(again, Ok(()));
        }

        #[test]
        fn says_why_it_failed() {
            let dir = std::env::temp_dir().join(format!(
                "maxhq-backup-missing-{}",
                maxhq_protocol::ids::InstanceId::generate()
            ));
            assert!(exclude(&dir).is_err());
        }
    }
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
mod system {
    use std::path::Path;

    pub fn exclude(_dir: &Path) -> Result<(), String> {
        Ok(())
    }
}
