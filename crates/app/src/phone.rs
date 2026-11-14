// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the phone build of the app has that the desktop's doesn't: a shell
//! that opens web links through the host app, and the links to the devices
//! in the book it paired with.
//!
//! Each device in the book gets one link, derived from the device's ID so
//! it stays the same from one book to the next. Its routes are its network
//! access first, `wss://host:port`, and then SSH URLs,
//! `ssh://user@host:port`, since the phone has no SSH config to resolve an
//! alias with. [`parse_route`] reads an SSH one back for the transport, and
//! the book in the registry gives the host key to pin.
//!
//! A device that asks has each connection prove it holds the phone's key,
//! with [`PhoneProver`], as the app proves its own.

use std::sync::Arc;

use futures::future::BoxFuture;
use maxhq_client::proof::{Kind, Prover, Proving, Verdict};
use maxhq_client::LinkError;
use maxhq_protocol::ids::{DeviceId, LinkId};
use maxhq_protocol::message::ActionOutcome;
#[cfg(feature = "phone")]
use maxhq_protocol::netaccess::PhoneNetwork;
use maxhq_protocol::phones::{PhoneBook, PhoneBookEntry, PhoneRoute};
#[cfg(feature = "phone")]
use maxhq_protocol::Secret;
use zeroize::Zeroizing;

use crate::app_keys::{reason_of, UNKNOWN_KEY};
use crate::{Opening, Shell};

/// Opens a link through the host app, which on iOS hands it to the system.
pub type OpenLink = Box<dyn Fn(String) -> Opening + Send + Sync>;

/// The phone's shell. It opens web pages through the host app and nothing
/// else: the phone has no editors.
pub struct PhoneShell {
    open: OpenLink,
}

impl PhoneShell {
    /// A shell that opens each web link with `open`.
    pub fn new(open: impl Fn(String) -> Opening + Send + Sync + 'static) -> Self {
        Self {
            open: Box::new(open),
        }
    }

    fn open_web(&self, url: String) -> Opening {
        if !is_web(&url) {
            return Box::pin(async { Err("The phone only opens web links".to_owned()) });
        }
        (self.open)(url)
    }
}

impl Shell for PhoneShell {
    fn open_browser(&self, url: String) -> Opening {
        self.open_web(url)
    }

    fn open_url(&self, url: String) -> Opening {
        self.open_web(url)
    }

    #[cfg(feature = "desktop")]
    fn spawn_editor(&self, _program: std::path::PathBuf, _args: Vec<String>) -> Opening {
        Box::pin(async { Err("The phone has no editors".to_owned()) })
    }

    #[cfg(feature = "desktop")]
    fn app_data(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    #[cfg(feature = "desktop")]
    fn quit(&self) {}
}

/// Signs the phone in to one network route of a device: the headers each
/// request on it carries, and what to do when the device ends the sign-in.
/// It gets the device's one-time code while the book still has one.
#[cfg(feature = "phone")]
pub type SignIn = Box<
    dyn Fn(
            DeviceId,
            &maxhq_client::WsRoute,
            Option<&Secret>,
        ) -> (maxhq_client::Headers, maxhq_client::Ended)
        + Send
        + Sync,
>;

/// The phone's way to its devices: the client over each device's network
/// access, pinned to its certificate, and over the phone's own SSH, pinned
/// to its host key.
#[cfg(feature = "phone")]
pub struct PhoneConnector {
    client: std::sync::Arc<maxhq_client::Client>,
    ssh: maxhq_client::PhoneSsh,
    ws: maxhq_client::PhoneWs,
    sign_in: SignIn,
}

#[cfg(feature = "phone")]
impl PhoneConnector {
    /// Starts the client on `runtime`, logging in over SSH with the
    /// ed25519 key made from `seed` and over the network with `sign_in`,
    /// with downloads and media in their folders as on the Mac. Each
    /// connection proves it holds that key to a device that asks.
    pub fn new(
        runtime: &tokio::runtime::Handle,
        seed: &[u8; 32],
        sign_in: SignIn,
        downloads: Option<std::path::PathBuf>,
        media: Option<std::path::PathBuf>,
    ) -> Self {
        let ssh = maxhq_client::PhoneSsh::new(seed);
        let ws = maxhq_client::PhoneWs::new();
        let transport = maxhq_client::Transport::Phone {
            ssh: ssh.clone(),
            ws: ws.clone(),
        };
        let client = crate::devices::start_client(
            runtime,
            || maxhq_client::Client::app(transport).proving(PhoneProver::phone(seed)),
            downloads,
            media,
        );
        Self {
            client: std::sync::Arc::new(client),
            ssh,
            ws,
            sign_in,
        }
    }

    /// The client, for the app's wakeups: suspend, wake and the network.
    pub fn client(&self) -> std::sync::Arc<maxhq_client::Client> {
        std::sync::Arc::clone(&self.client)
    }

    /// The phone key's public half as an OpenSSH line.
    pub fn public_key(&self) -> String {
        self.ssh.public_key()
    }
}

#[cfg(feature = "phone")]
impl crate::devices::Connector for PhoneConnector {
    type Link = maxhq_client::LinkHandle;

    fn open(
        &self,
        spec: maxhq_client::LinkSpec,
        device: Option<DeviceId>,
    ) -> (
        maxhq_client::LinkHandle,
        tokio::sync::mpsc::Receiver<maxhq_client::LinkEvent>,
    ) {
        crate::devices::Connector::open(&*self.client, spec, device)
    }

    fn watch_networks(
        &self,
        on_change: Box<dyn Fn(Vec<maxhq_protocol::link::Network>) + Send + Sync>,
    ) {
        crate::devices::Connector::watch_networks(&*self.client, on_change);
    }

    fn trust_book(&self, book: &PhoneBook) {
        for entry in &book.devices {
            if let Some(network) = &entry.network {
                for route in ws_routes(network) {
                    let (headers, ended) =
                        (self.sign_in)(entry.device, &route, network.code.as_ref());
                    self.ws.trust(&[route], &headers, &ended);
                }
            }
            self.ssh.trust(entry);
        }
    }
}

/// A device's network access as the routes the phone dials, one per host.
#[cfg(feature = "phone")]
pub fn ws_routes(network: &PhoneNetwork) -> Vec<maxhq_client::WsRoute> {
    network
        .hosts
        .iter()
        .map(|host| maxhq_client::WsRoute {
            host: host.clone(),
            port: network.port,
            pin: network.pin.clone(),
        })
        .collect()
}

/// What the phone says, after a device's name, once the device turns down
/// the phone's key. The shared view reads it to offer pairing again.
pub const NO_LONGER_KNOWN: &str = "no longer knows this phone. Pair it again from the Mac.";

/// Proves the phone's connections with one key: the phone's own SSH key,
/// or while it pairs, the code's one-time key.
pub struct PhoneProver {
    kind: Kind,
    seed: Zeroizing<[u8; 32]>,
}

impl PhoneProver {
    /// Proves the phone with its SSH key, made from `seed`.
    pub fn phone(seed: &[u8; 32]) -> Arc<dyn Prover> {
        Self::with_key(Kind::Phone, seed)
    }

    /// Proves a pairing with the code's one-time key, made from `seed`.
    pub fn pairing(seed: &[u8; 32]) -> Arc<dyn Prover> {
        Self::with_key(Kind::Pair, seed)
    }

    fn with_key(kind: Kind, seed: &[u8; 32]) -> Arc<dyn Prover> {
        Arc::new(Self {
            kind,
            seed: Zeroizing::new(*seed),
        })
    }
}

impl Prover for PhoneProver {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict> {
        let (kind, seed) = (self.kind, self.seed.clone());
        Box::pin(async move {
            // A device knows the phone it paired with, so the Sync goes
            // right behind. A pairing syncs nothing.
            let answer = proving.prove(&seed, kind == Kind::Phone).await;
            verdict(kind, proving.name(), answer)
        })
    }
}

/// What the phone makes of `name`'s answer to its proof.
fn verdict(kind: Kind, name: &str, answer: Result<ActionOutcome, LinkError>) -> Verdict {
    match answer {
        Ok(ActionOutcome::Done) => Verdict::Proven,
        Ok(refused) if kind == Kind::Pair => Verdict::Refused(format!(
            "{name} didn't take this pairing code, so scan a new one: {}",
            reason_of(refused)
        )),
        Ok(ActionOutcome::Refused { reason }) if reason.starts_with(UNKNOWN_KEY) => {
            Verdict::Refused(format!("{name} {NO_LONGER_KNOWN}"))
        }
        Ok(refused) => Verdict::Refused(format!(
            "{name} didn't take this phone's proof: {}",
            reason_of(refused)
        )),
        Err(error) => {
            tracing::debug!(%error, "a proof ended with its connection");
            Verdict::Lost
        }
    }
}

/// Whether `url` is a web page: `http` or `https`.
pub fn is_web(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

/// The link the phone reaches `device` on.
pub fn book_link(device: DeviceId) -> LinkId {
    LinkId::derive(&format!("phone-book:{device}"))
}

/// A route as the host a link dials: `ssh://user@host:port`, with an IPv6
/// address in brackets.
pub use maxhq_client::phone_route_host as route_host;

/// The hosts a link dials for `entry`, each once: its network access
/// first, as `wss://host:port`, and then its SSH routes.
pub fn route_hosts(entry: &PhoneBookEntry) -> Vec<String> {
    let network = entry.network.iter().flat_map(|network| {
        let port = network.port;
        network
            .hosts
            .iter()
            .map(move |host| maxhq_client::ws_route_host(host, port))
    });
    let mut hosts: Vec<String> = Vec::new();
    for host in network.chain(entry.routes.iter().map(route_host)) {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// Reads a host that [`route_host`] wrote, or None for any other.
pub fn parse_route(host: &str) -> Option<PhoneRoute> {
    let rest = host.strip_prefix("ssh://")?;
    let (user, rest) = rest.split_once('@')?;
    let (host, port) = rest.rsplit_once(':')?;
    let host = match host.strip_prefix('[') {
        Some(inner) => inner.strip_suffix(']')?,
        None if host.contains(':') => return None,
        None => host,
    };
    if user.is_empty() || host.is_empty() {
        return None;
    }
    Some(PhoneRoute {
        user: user.to_owned(),
        host: host.to_owned(),
        port: port.parse().ok()?,
    })
}

/// The host key the book pins for the device on `link`.
pub fn host_key(book: &PhoneBook, link: LinkId) -> Option<&str> {
    book.devices
        .iter()
        .find(|entry| book_link(entry.device) == link)
        .map(|entry| entry.host_key.as_str())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use maxhq_protocol::phones::PhoneBookEntry;

    use super::*;
    use crate::tests::block_on;

    fn route(user: &str, host: &str, port: u16) -> PhoneRoute {
        PhoneRoute {
            user: user.into(),
            host: host.into(),
            port,
        }
    }

    /// A device in a book, reached as `dev` at each of `hosts`.
    pub(crate) fn entry(name: &str, hosts: &[&str]) -> PhoneBookEntry {
        PhoneBookEntry {
            device: DeviceId::derive(name),
            name: name.into(),
            routes: hosts.iter().map(|host| route("dev", host, 22)).collect(),
            host_key: "ssh-ed25519 AAAAexample".into(),
            network: None,
        }
    }

    /// The test phone's book, from the first device in it.
    pub(crate) fn book(devices: Vec<PhoneBookEntry>) -> PhoneBook {
        PhoneBook {
            phone: maxhq_protocol::ids::PhoneId::derive("phone"),
            device: devices
                .first()
                .map_or(DeviceId::derive("none"), |entry| entry.device),
            devices,
        }
    }

    #[test]
    fn routes_read_back() {
        for route in [
            route("dev", "devbox.local", 22),
            route("dev", "192.0.2.7", 2222),
            route("dev", "2001:db8::7", 22),
        ] {
            let host = route_host(&route);
            assert!(host.starts_with("ssh://dev@"), "{host}");
            assert_eq!(parse_route(&host), Some(route));
        }
        assert_eq!(
            route_host(&route("dev", "2001:db8::7", 22)),
            "ssh://dev@[2001:db8::7]:22"
        );
        for bad in [
            "dev@devbox",
            "ssh://devbox:22",
            "ssh://dev@devbox",
            "ssh://dev@2001:db8::7:22",
            "ssh://@devbox:22",
            "ssh://dev@devbox:port",
        ] {
            assert_eq!(parse_route(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_book_pins_each_link_its_host_key() {
        let devbox = DeviceId::derive("devbox");
        let book = book(vec![entry("devbox", &["devbox.local"])]);
        assert_eq!(
            host_key(&book, book_link(devbox)),
            Some("ssh-ed25519 AAAAexample")
        );
        assert_eq!(host_key(&book, book_link(DeviceId::derive("other"))), None);
    }

    #[test]
    fn a_device_that_turns_the_phone_down_says_to_pair_again() {
        let refused = |reason: &str| {
            Ok(ActionOutcome::Refused {
                reason: reason.into(),
            })
        };
        assert_eq!(
            verdict(Kind::Phone, "devbox", Ok(ActionOutcome::Done)),
            Verdict::Proven
        );
        assert_eq!(
            verdict(
                Kind::Phone,
                "devbox",
                refused("Unknown key: devbox doesn't know this phone's key.")
            ),
            Verdict::Refused(
                "devbox no longer knows this phone. Pair it again from the Mac.".into()
            )
        );
        // Any other refusal shows the device's reason, without the prefix
        // maxd sorts it by.
        assert_eq!(
            verdict(
                Kind::Phone,
                "devbox",
                refused("Bad proof: the signature doesn't match.")
            ),
            Verdict::Refused(
                "devbox didn't take this phone's proof: the signature doesn't match.".into()
            )
        );
        assert_eq!(
            verdict(
                Kind::Pair,
                "the device",
                refused("Unknown key: that isn't this pairing's key.")
            ),
            Verdict::Refused(
                "the device didn't take this pairing code, so scan a new one: that isn't this \
                 pairing's key."
                    .into()
            )
        );
        assert_eq!(
            verdict(Kind::Phone, "devbox", Err(LinkError::NotConnected)),
            Verdict::Lost
        );
    }

    #[test]
    fn the_shell_opens_web_links_only() {
        let opened = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&opened);
        let shell = PhoneShell::new(move |url| {
            kept.lock().unwrap().push(url);
            Box::pin(async { Ok(()) })
        });
        block_on(shell.open_browser("https://example.com/".into())).unwrap();
        block_on(shell.open_url("HTTP://example.com/a".into())).unwrap();
        for refused in ["cursor://file/tmp", "file:///etc/hosts", "example.com"] {
            assert!(
                block_on(shell.open_url(refused.into())).is_err(),
                "{refused}"
            );
        }
        #[cfg(feature = "desktop")]
        assert!(block_on(shell.spawn_editor("/bin/true".into(), Vec::new())).is_err());
        assert_eq!(
            *opened.lock().unwrap(),
            ["https://example.com/", "HTTP://example.com/a"]
        );
    }
}
