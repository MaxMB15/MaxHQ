// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's own SSH key, which its peer links log in to other devices
//! with. It's made the first time it's needed, kept 0600 in a 0700 folder,
//! and only its public half ever leaves this module.

use std::io;

use ring::rand::{SecureRandom, SystemRandom};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey};
use zeroize::Zeroizing;

use crate::secrets::files::Folder;

/// The key's file name in the folder.
pub const KEY_FILE: &str = "id_ed25519";

/// The comment the private key carries.
const KEY_COMMENT: &str = "maxhq-device";

/// The public half of the key in `folder`, as `ssh-ed25519 <base64>`,
/// after making the key if there's none.
pub fn public_key(folder: &Folder) -> io::Result<String> {
    let key = match read(folder)? {
        Some(key) => key,
        None => make(folder)?,
    };
    let mut public = key.public_key().clone();
    public.set_comment("");
    public.to_openssh().map_err(io::Error::other)
}

/// The 32 bytes the key in `folder` is made from, which sign the proof a
/// peer link gives, or none when there's no key yet. They stay in this
/// process.
pub fn seed(folder: &Folder) -> io::Result<Option<Zeroizing<[u8; 32]>>> {
    let Some(key) = read(folder)? else {
        return Ok(None);
    };
    let pair = key.key_data().ed25519().ok_or_else(unreadable)?;
    Ok(Some(Zeroizing::new(pair.private.to_bytes())))
}

/// The key in `folder`, checked to be one unencrypted ed25519 key, or
/// none when there's no file.
fn read(folder: &Folder) -> io::Result<Option<PrivateKey>> {
    let Some(text) = folder.read(KEY_FILE)? else {
        return Ok(None);
    };
    let key = PrivateKey::from_openssh(text.expose().as_bytes()).map_err(|_| unreadable())?;
    if key.algorithm() != Algorithm::Ed25519 || key.is_encrypted() {
        return Err(unreadable());
    }
    Ok(Some(key))
}

/// Makes a new key from the system's random source and writes it as
/// OpenSSH does.
fn make(folder: &Folder) -> io::Result<PrivateKey> {
    let mut seed = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(seed.as_mut())
        .map_err(|_| io::Error::other("the system gave no random bytes"))?;
    let mut key = PrivateKey::from(Ed25519Keypair::from_seed(&seed));
    key.set_comment(KEY_COMMENT);
    let text = key.to_openssh(LineEnding::LF).map_err(io::Error::other)?;
    folder.write(KEY_FILE, text.as_bytes())?;
    Ok(key)
}

/// The error for a key file that isn't one unencrypted ed25519 key. It
/// names nothing in the file.
fn unreadable() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "the device's SSH key isn't one maxd can read",
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("maxhq-peer-key-{}", uuid::Uuid::now_v7()))
    }

    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).expect("meta").permissions().mode() & 0o777
    }

    #[test]
    fn the_key_is_made_once_0600_in_a_0700_folder() {
        let dir = temp();
        let folder = Folder::open(dir.join("ssh")).expect("folder");
        let public = public_key(&folder).expect("made");
        assert!(public.starts_with("ssh-ed25519 "), "{public}");
        assert_eq!(public.split_whitespace().count(), 2);
        assert_eq!(mode(&dir.join("ssh")), 0o700);
        assert_eq!(mode(&dir.join("ssh").join(KEY_FILE)), 0o600);
        // The same key comes back, and no other file holds it.
        assert_eq!(public_key(&folder).expect("read"), public);
        let names: Vec<_> = fs::read_dir(dir.join("ssh"))
            .expect("list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, [KEY_FILE]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_key_file_is_refused_without_showing_it() {
        let dir = temp();
        let folder = Folder::open(dir.join("ssh")).expect("folder");
        folder.write(KEY_FILE, b"not a key at all").expect("write");
        let err = public_key(&folder).expect_err("refused");
        assert!(!err.to_string().contains("not a key"), "{err}");
        assert!(seed(&folder).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_seed_signs_for_the_public_key() {
        let dir = temp();
        let folder = Folder::open(dir.join("ssh")).expect("folder");
        assert!(seed(&folder).expect("no key yet").is_none());
        let public = public_key(&folder).expect("made");
        let seed = seed(&folder).expect("read").expect("a key");
        let statement = maxhq_core::prove::Statement {
            kind: maxhq_core::prove::Kind::Peer,
            device: maxhq_protocol::ids::DeviceId::derive("device"),
            instance: maxhq_protocol::ids::InstanceId::derive("run"),
            client: maxhq_protocol::ids::ClientId::derive("client"),
            challenge: maxhq_core::prove::Challenge::generate().expect("random"),
        };
        let proof = maxhq_core::prove::sign(&seed, &statement).expect("signed");
        let signer = maxhq_core::prove::verify(&proof.public_key, &proof.signature, &statement)
            .expect("verified");
        assert_eq!(crate::sshkey::ed25519_key(&public), Some(signer));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_private_key_never_reaches_the_log() {
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().expect("log").extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let log = Buffer::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        let dir = temp();
        tracing::subscriber::with_default(subscriber, || {
            let folder = Folder::open(dir.join("ssh")).expect("folder");
            public_key(&folder).expect("made");
            public_key(&folder).expect("read");
        });
        let private = fs::read_to_string(dir.join("ssh").join(KEY_FILE)).expect("key");
        let body: String = private
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let logged = String::from_utf8(log.0.lock().expect("log").clone()).expect("text");
        assert!(!logged.contains(&body[..40]));
        let _ = fs::remove_dir_all(dir);
    }
}
