// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The code the Mac shows to pair a phone, as a QR code, and the phone
//! reads back: the pairing, its one-time key, and how to reach the device
//! it pairs through.
//!
//! It is `maxhq:` and then base64url, with no padding, of:
//!
//! - a version byte, 1
//! - the pair ID, 16 bytes
//! - the one-time key's Ed25519 seed, 32 bytes
//! - the device ID, 16 bytes
//! - the device's Ed25519 host key, 32 bytes
//! - a route count, then for each route the user and the host, each as a
//!   length byte and its bytes, and the port, 2 bytes big-endian
//!
//! A device with a few routes fits in under 300 bytes.
//!
//! Version 2 is for a device whose network access is on. After the device
//! ID come the route count, the host key only when there are routes, and
//! the routes as above. Then the device's network part: a host count and
//! each host as a length byte and its bytes, the port, 2 bytes big-endian,
//! and the certificate pin's 32 bytes. A device with no other way in,
//! such as a Mac, fits in under 250 bytes.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::ids::{DeviceId, PairId};
use maxhq_protocol::netaccess::PhoneNetwork;
use maxhq_protocol::phones::PhoneRoute;
use ssh_key::public::{Ed25519PublicKey, KeyData};
use uuid::Uuid;
use zeroize::Zeroizing;

/// What every code starts with.
const PREFIX: &str = "maxhq:";

/// The layout's version.
const VERSION: u8 = 1;

/// The layout's version with a network part.
const NETWORK_VERSION: u8 = 2;

/// The most routes a code carries, and the most network hosts. The phone
/// gets the rest in its book.
pub const ROUTES_MAX: usize = 4;

/// What a pairing code holds.
#[derive(Clone, PartialEq, Eq)]
pub struct PairCode {
    /// The pairing, for `maxd proxy --pair`.
    pub pair: PairId,
    /// The one-time key's Ed25519 seed.
    pub seed: Zeroizing<[u8; 32]>,
    /// The device the phone pairs through.
    pub device: DeviceId,
    /// Its SSH host key, as `ssh-ed25519 <base64>`, or empty when it has
    /// no routes.
    pub host_key: String,
    /// Where the phone dials it over SSH, in order.
    pub routes: Vec<PhoneRoute>,
    /// Where it listens for phones over the network, when its switch is
    /// on. It never carries a code.
    pub network: Option<PhoneNetwork>,
}

impl std::fmt::Debug for PairCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairCode")
            .field("pair", &self.pair)
            .field("device", &self.device)
            .field("routes", &self.routes)
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl PairCode {
    /// The code as text, as version 1 unless it has a network part. It
    /// fails on routes with a host key that isn't Ed25519, a user or host
    /// too long for a length byte, a pin that isn't 32 bytes, or no way to
    /// reach the device at all.
    pub fn encode(&self) -> Result<Zeroizing<String>, String> {
        let routes = &self.routes[..self.routes.len().min(ROUTES_MAX)];
        let mut bytes = Zeroizing::new(Vec::with_capacity(256));
        bytes.push(match self.network {
            Some(_) => NETWORK_VERSION,
            None => VERSION,
        });
        bytes.extend_from_slice(self.pair.0.as_bytes());
        bytes.extend_from_slice(self.seed.as_slice());
        bytes.extend_from_slice(self.device.0.as_bytes());
        let host_key = || {
            ed25519(&self.host_key)
                .ok_or_else(|| "The device's host key isn't an Ed25519 key".to_owned())
        };
        match &self.network {
            None if routes.is_empty() => return Err(NO_WAY.to_owned()),
            None => {
                bytes.extend_from_slice(&host_key()?);
                bytes.push(routes.len() as u8);
            }
            Some(_) => {
                bytes.push(routes.len() as u8);
                if !routes.is_empty() {
                    bytes.extend_from_slice(&host_key()?);
                }
            }
        }
        for route in routes {
            push_text(&mut bytes, &route.user)?;
            push_text(&mut bytes, &route.host)?;
            bytes.extend_from_slice(&route.port.to_be_bytes());
        }
        if let Some(network) = &self.network {
            let hosts = &network.hosts[..network.hosts.len().min(ROUTES_MAX)];
            if hosts.is_empty() {
                return Err(NO_WAY.to_owned());
            }
            bytes.push(hosts.len() as u8);
            for host in hosts {
                push_text(&mut bytes, host)?;
            }
            bytes.extend_from_slice(&network.port.to_be_bytes());
            let pin = URL_SAFE_NO_PAD
                .decode(&network.pin)
                .ok()
                .filter(|pin| pin.len() == 32)
                .ok_or_else(|| "The device's certificate pin isn't 32 bytes".to_owned())?;
            bytes.extend_from_slice(&pin);
        }
        let mut text = Zeroizing::new(String::from(PREFIX));
        URL_SAFE_NO_PAD.encode_string(bytes.as_slice(), &mut text);
        Ok(text)
    }

    /// Reads a code back, either version, or says why it isn't one.
    pub fn decode(text: &str) -> Result<Self, String> {
        const NOT_ONE: &str = "That isn't a MaxHQ pairing code";
        let body = text.trim().strip_prefix(PREFIX).ok_or(NOT_ONE)?;
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(body).map_err(|_| NOT_ONE)?);
        let mut read = Reader(bytes.as_slice());
        let networked = match read.byte().ok_or(NOT_ONE)? {
            VERSION => false,
            NETWORK_VERSION => true,
            _ => return Err("That pairing code is from a newer MaxHQ. Update the app.".into()),
        };
        let pair = PairId(Uuid::from_bytes(read.array().ok_or(NOT_ONE)?));
        let seed = Zeroizing::new(read.array().ok_or(NOT_ONE)?);
        let device = DeviceId(Uuid::from_bytes(read.array().ok_or(NOT_ONE)?));
        let host_key = |read: &mut Reader<'_>| {
            let key: [u8; 32] = read.array().ok_or(NOT_ONE)?;
            ssh_key::PublicKey::new(KeyData::Ed25519(Ed25519PublicKey(key)), "")
                .to_openssh()
                .map_err(|_| NOT_ONE)
        };
        let (host_key, count) = if networked {
            let count = read.byte().ok_or(NOT_ONE)?;
            let key = if count > 0 {
                host_key(&mut read)?
            } else {
                String::new()
            };
            (key, count)
        } else {
            let key = host_key(&mut read)?;
            (key, read.byte().ok_or(NOT_ONE)?)
        };
        let mut routes = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let user = read.text().ok_or(NOT_ONE)?;
            let host = read.text().ok_or(NOT_ONE)?;
            let port = u16::from_be_bytes(read.array().ok_or(NOT_ONE)?);
            routes.push(PhoneRoute { user, host, port });
        }
        let network = if networked {
            let count = read.byte().filter(|&count| count > 0).ok_or(NOT_ONE)?;
            let hosts = (0..count)
                .map(|_| read.text().ok_or(NOT_ONE))
                .collect::<Result<Vec<_>, _>>()?;
            let port = u16::from_be_bytes(read.array().ok_or(NOT_ONE)?);
            let pin: [u8; 32] = read.array().ok_or(NOT_ONE)?;
            Some(PhoneNetwork {
                hosts,
                port,
                pin: URL_SAFE_NO_PAD.encode(pin),
                code: None,
            })
        } else {
            None
        };
        if !read.0.is_empty() || (routes.is_empty() && network.is_none()) {
            return Err(NOT_ONE.into());
        }
        Ok(Self {
            pair,
            seed,
            device,
            host_key,
            routes,
            network,
        })
    }
}

/// Why a code with no routes and no network part can't be made.
const NO_WAY: &str = "The pairing code has no way to reach the device";

/// Adds `text` as a length byte and its bytes.
fn push_text(bytes: &mut Vec<u8>, text: &str) -> Result<(), String> {
    let len = u8::try_from(text.len())
        .map_err(|_| format!("The address {text} is too long for a pairing code"))?;
    bytes.push(len);
    bytes.extend_from_slice(text.as_bytes());
    Ok(())
}

/// The 32 bytes of an `ssh-ed25519 <base64>` key.
fn ed25519(line: &str) -> Option<[u8; 32]> {
    let key = ssh_key::PublicKey::from_openssh(line.trim()).ok()?;
    Some(key.key_data().ed25519()?.0)
}

/// Reads a code's bytes from the front.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (front, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(front)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn text(&mut self) -> Option<String> {
        let len = self.byte()?;
        let text = std::str::from_utf8(self.take(usize::from(len))?).ok()?;
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// A QR code as an SVG path, one unit a module, with no margin.
#[cfg(feature = "desktop")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct QrPath {
    /// The modules on a side.
    pub size: u32,
    /// The dark modules, as an SVG path's `d`.
    pub path: String,
}

/// `text` as a QR code at medium error correction.
#[cfg(feature = "desktop")]
pub fn qr_path(text: &str) -> Result<QrPath, String> {
    use std::fmt::Write;

    let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium)
        .map_err(|_| "The pairing code is too long for a QR code".to_owned())?;
    let size = code.size();
    let mut path = String::new();
    // Each row's dark runs, as one rectangle each.
    for y in 0..size {
        let mut x = 0;
        while x < size {
            if !code.get_module(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < size && code.get_module(x, y) {
                x += 1;
            }
            let _ = write!(path, "M{start} {y}h{}v1h-{}z", x - start, x - start);
        }
    }
    Ok(QrPath {
        size: size.unsigned_abs(),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up host key.
    pub(crate) const HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";

    fn route(user: &str, host: &str, port: u16) -> PhoneRoute {
        PhoneRoute {
            user: user.into(),
            host: host.into(),
            port,
        }
    }

    pub(crate) fn code() -> PairCode {
        PairCode {
            pair: PairId::derive("pairing"),
            seed: Zeroizing::new([7; 32]),
            device: DeviceId::derive("devbox"),
            host_key: HOST_KEY.into(),
            routes: vec![
                route("dev", "devbox.local", 22),
                route("dev", "devbox.example.ts.net", 22),
                route("dev", "2001:db8::7", 2222),
            ],
            network: None,
        }
    }

    /// Network access on `hosts`, with a made-up pin.
    fn network(hosts: &[&str]) -> PhoneNetwork {
        PhoneNetwork {
            hosts: hosts.iter().map(|&host| host.to_owned()).collect(),
            port: 7466,
            pin: URL_SAFE_NO_PAD.encode([9; 32]),
            code: None,
        }
    }

    #[test]
    fn a_code_reads_back_and_fits() {
        let code = code();
        let text = code.encode().unwrap();
        assert!(text.starts_with("maxhq:"), "{}", text.as_str());
        assert!(text.len() < 300, "{} bytes", text.len());
        assert_eq!(PairCode::decode(&text).unwrap(), code);
        let printed = format!("{code:?}");
        assert!(!printed.contains("seed"), "{printed}");
        assert!(text.starts_with("maxhq:A"), "version 1: {}", text.as_str());
    }

    #[test]
    fn a_code_with_network_access_reads_back_and_fits() {
        let mut code = code();
        code.network = Some(network(&["192.168.1.20", "fd7a:115c:a1e0::1"]));
        let text = code.encode().unwrap();
        assert!(text.len() < 400, "{} bytes", text.len());
        assert_eq!(PairCode::decode(&text).unwrap(), code);

        // A Mac has no SSH, so its code has network access alone.
        let mac = PairCode {
            host_key: String::new(),
            routes: Vec::new(),
            network: Some(network(&[
                "192.168.1.20",
                "100.101.102.103",
                "fd7a:115c:a1e0::1",
                "fd12:3456:789a:1::20",
            ])),
            ..code
        };
        let text = mac.encode().unwrap();
        assert!(text.len() < 250, "{} bytes", text.len());
        assert_eq!(PairCode::decode(&text).unwrap(), mac);
        // Small enough to scan from the Mac's screen.
        #[cfg(feature = "desktop")]
        assert!(qr_path(&text).unwrap().size <= 65);
    }

    #[test]
    fn a_code_needs_some_way_in() {
        let mut code = code();
        code.routes.clear();
        assert!(code.encode().unwrap_err().contains("no way"));
        let mut empty = network(&[]);
        code.network = Some(empty.clone());
        assert!(code.encode().is_err());
        empty.hosts.push("192.168.1.20".into());
        empty.pin = "short".into();
        code.network = Some(empty);
        assert!(code.encode().unwrap_err().contains("pin"));
    }

    #[test]
    fn a_code_carries_only_the_first_few_routes() {
        let mut code = code();
        code.routes = (0..6)
            .map(|n| route("dev", &format!("host{n}"), 22))
            .collect();
        let back = PairCode::decode(&code.encode().unwrap()).unwrap();
        assert_eq!(back.routes, code.routes[..ROUTES_MAX]);
    }

    #[test]
    fn anything_else_is_not_a_code() {
        let text = code().encode().unwrap();
        let cut = &text[..text.len() - 4];
        let longer = format!("{}AAAA", text.as_str());
        for bad in [
            "",
            "https://example.com",
            "maxhq:",
            "maxhq:!!",
            cut,
            &longer,
        ] {
            assert!(PairCode::decode(bad).is_err(), "{bad}");
        }
        let mut newer = URL_SAFE_NO_PAD
            .decode(text.strip_prefix(PREFIX).unwrap())
            .unwrap();
        newer[0] = 3;
        let newer = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(newer));
        assert!(PairCode::decode(&newer).unwrap_err().contains("newer"));
        let mut rsa = code();
        rsa.host_key = "ssh-rsa AAAAB3NzaC1yc2E".into();
        assert!(rsa.encode().is_err());
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn the_qr_code_is_square_rows_of_runs() {
        let text = code().encode().unwrap();
        let qr = qr_path(&text).unwrap();
        assert!(qr.size >= 21 && qr.size <= 177, "{}", qr.size);
        assert!(qr.path.starts_with("M0 0h7v1h-7z"), "{}", &qr.path[..20]);
    }
}
