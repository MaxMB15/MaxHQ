// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's own TLS certificate for network access: self-signed, made
//! once and kept in a 0600 file, so its pin stays the same across
//! restarts. Phones pin it at pairing, so no authority signs it.

use std::io;
use std::sync::Arc;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::digest::{digest, SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use serde::{Deserialize, Serialize};

use crate::secrets::files::Folder;

/// The file in the folder.
const FILE: &str = "tls.json";
/// The name in the certificate. Nothing checks it, since phones pin it.
const NAME: &str = "maxd";

/// What the listeners serve with.
#[derive(Clone)]
pub struct Tls {
    /// The server's rustls config.
    pub config: Arc<ServerConfig>,
    /// The certificate's pin, as `NetworkAccess::pin`.
    pub pin: String,
}

/// The certificate and its key in the file, as base64 of their DER.
#[derive(Serialize, Deserialize)]
struct Kept {
    cert: String,
    key: String,
}

/// The pin of a certificate: base64url, without padding, of the SHA-256
/// of its DER.
pub fn pin(cert: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(digest(&SHA256, cert))
}

/// The certificate `folder` keeps, or a new one, kept there first.
pub fn load(folder: &Folder) -> io::Result<Tls> {
    let (cert, key) = match folder.read(FILE)? {
        Some(text) => {
            let kept: Kept = serde_json::from_str(text.expose()).map_err(io::Error::other)?;
            let decode = |part: &str| STANDARD.decode(part).map_err(io::Error::other);
            (decode(&kept.cert)?, decode(&kept.key)?)
        }
        None => {
            let made = rcgen::generate_simple_self_signed(vec![NAME.to_owned()])
                .map_err(io::Error::other)?;
            let cert = made.cert.der().to_vec();
            let key = made.signing_key.serialize_der();
            let kept = Kept {
                cert: STANDARD.encode(&cert),
                key: STANDARD.encode(&key),
            };
            folder.write(FILE, &serde_json::to_vec(&kept).map_err(io::Error::other)?)?;
            (cert, key)
        }
    };
    let pin = pin(&cert);
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .and_then(|config| {
                config.with_no_client_auth().with_single_cert(
                    vec![CertificateDer::from(cert)],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
                )
            })
            .map_err(io::Error::other)?;
    Ok(Tls {
        config: Arc::new(config),
        pin,
    })
}

#[cfg(test)]
mod tests {
    use maxhq_os::fsmode;

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn the_certificate_is_made_once_and_kept_private() {
        let dir = TempDir::new();
        let folder = Folder::open(dir.path().join("netaccess")).unwrap();
        let first = load(&folder).unwrap();
        let again = load(&folder).unwrap();
        assert_eq!(first.pin, again.pin);
        assert_eq!(first.pin.len(), 43);
        let mode =
            fsmode::mode_of(&std::fs::metadata(dir.path().join("netaccess").join(FILE)).unwrap());
        assert_eq!(mode & 0o777, 0o600);
    }
}
