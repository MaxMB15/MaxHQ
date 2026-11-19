// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the phone build of the app has that the desktop's doesn't: a shell
//! that opens web links through the host app, and the links to the devices
//! in the books it paired with, one book per Mac.
//!
//! Each device in the books gets one link, derived from the device's ID so
//! it stays the same from one book to the next. A device two Macs both list
//! gets one link, under the first Mac that listed it. Its routes are its network
//! access first, `wss://host:port`, and then SSH URLs,
//! `ssh://user@host:port`, since the phone has no SSH config to resolve an
//! alias with. [`parse_route`] reads an SSH one back for the transport, and
//! the book in the registry gives the host key to pin.
//!
//! A device that asks has each connection prove it holds the phone's key,
//! with [`PhoneProver`], as the app proves its own.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

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
/// It gets the name of the Mac whose book lists the device, and the
/// device's one-time code while the book still has one.
#[cfg(feature = "phone")]
pub type SignIn = Box<
    dyn Fn(
            DeviceId,
            &str,
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
    macs: MacNames,
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
        let macs = MacNames::default();
        let naming = macs.clone();
        let transport = maxhq_client::Transport::Phone {
            ssh: ssh.clone(),
            ws: ws.clone(),
        };
        let client = crate::devices::start_client(
            runtime,
            || maxhq_client::Client::app(transport).proving(PhoneProver::naming(seed, naming)),
            downloads,
            media,
        );
        Self {
            client: std::sync::Arc::new(client),
            ssh,
            ws,
            sign_in,
            macs,
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

    fn trust_books(&self, books: &[PhoneBook]) {
        self.macs.keep(books);
        for (at, entry) in listed(books) {
            let mac = mac_name(&books[at]);
            if let Some(network) = &entry.network {
                for route in ws_routes(network) {
                    let (headers, ended) =
                        (self.sign_in)(entry.device, &mac, &route, network.code.as_ref());
                    self.ws.trust(entry.device, &[route], &headers, &ended);
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

/// What the phone says between a device's name and the Mac's, once the
/// device turns down the phone's key, as [`no_longer_known`] puts them.
/// The shared view reads it to offer pairing again.
pub const NO_LONGER_KNOWN: &str = "no longer knows this phone. Pair it again from";

/// What the phone says once `name` turns down its key: to pair again from
/// `mac`, the Mac whose book lists it.
pub fn no_longer_known(name: &str, mac: &str) -> String {
    format!("{name} {NO_LONGER_KNOWN} {mac}.")
}

/// What the phone calls a Mac that didn't name itself.
pub const SOME_MAC: &str = "the Mac";

/// The keeper of the book the phone makes for the devices you add by hand.
/// No device has this ID, so no report ever speaks for that book.
pub fn by_hand() -> DeviceId {
    DeviceId::derive("phone-by-hand")
}

/// Each device the books list, once, with where the first book that lists
/// it is, in the books' order.
pub fn listed(books: &[PhoneBook]) -> Vec<(usize, &PhoneBookEntry)> {
    let mut found: Vec<(usize, &PhoneBookEntry)> = Vec::new();
    for (at, book) in books.iter().enumerate() {
        for entry in &book.devices {
            if found.iter().all(|(_, have)| have.device != entry.device) {
                found.push((at, entry));
            }
        }
    }
    found
}

/// Whether `a` and `b` are books from the same Mac: one keeper, or one Mac
/// that paired again through another keeper.
pub fn same_mac(a: &PhoneBook, b: &PhoneBook) -> bool {
    a.device == b.device || matches!((&a.mac, &b.mac), (Some(a), Some(b)) if a.device == b.device)
}

/// Puts `book` in `books` in place of the ones from its Mac, or else last.
pub fn put_book(books: &mut Vec<PhoneBook>, book: PhoneBook) {
    let at = books.iter().position(|have| same_mac(have, &book));
    books.retain(|have| !same_mac(have, &book));
    // Nothing before the first one left, so it's still in its place.
    books.insert(at.unwrap_or(books.len()), book);
}

/// The name of the Mac that made `book`: the one it gave, or for a book
/// from an older Mac its keeper's name in the book, or else "the Mac".
pub fn mac_name(book: &PhoneBook) -> String {
    book.mac
        .as_ref()
        .map(|mac| mac.name.clone())
        .or_else(|| {
            book.devices
                .iter()
                .find(|entry| entry.device == book.device)
                .map(|entry| entry.name.clone())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| SOME_MAC.to_owned())
}

/// The name of the Mac each device's book came from, for what the phone
/// says about the device. Clones share it.
#[derive(Clone, Default)]
pub struct MacNames(Arc<RwLock<BTreeMap<DeviceId, String>>>);

impl MacNames {
    /// Names each device in `books` by the first book that lists it.
    pub fn keep(&self, books: &[PhoneBook]) {
        let names = listed(books)
            .into_iter()
            .map(|(at, entry)| (entry.device, mac_name(&books[at])))
            .collect();
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = names;
    }

    /// The Mac whose book lists `device`, or "the Mac" for one no book does.
    pub fn of(&self, device: DeviceId) -> String {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&device)
            .cloned()
            .unwrap_or_else(|| SOME_MAC.to_owned())
    }
}

/// Proves the phone's connections with one key: the phone's own SSH key,
/// or while it pairs, the code's one-time key.
pub struct PhoneProver {
    kind: Kind,
    seed: Zeroizing<[u8; 32]>,
    macs: MacNames,
}

impl PhoneProver {
    /// Proves the phone with its SSH key, made from `seed`.
    pub fn phone(seed: &[u8; 32]) -> Arc<dyn Prover> {
        Self::naming(seed, MacNames::default())
    }

    /// Proves the phone with its SSH key, made from `seed`, and names the
    /// Mac to pair again from with `macs`.
    pub fn naming(seed: &[u8; 32], macs: MacNames) -> Arc<dyn Prover> {
        Arc::new(Self {
            kind: Kind::Phone,
            seed: Zeroizing::new(*seed),
            macs,
        })
    }

    /// Proves a pairing with the code's one-time key, made from `seed`.
    pub fn pairing(seed: &[u8; 32]) -> Arc<dyn Prover> {
        Arc::new(Self {
            kind: Kind::Pair,
            seed: Zeroizing::new(*seed),
            macs: MacNames::default(),
        })
    }
}

impl Prover for PhoneProver {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict> {
        let (kind, seed) = (self.kind, self.seed.clone());
        let mac = self.macs.of(proving.device());
        Box::pin(async move {
            // A device knows the phone it paired with, so the Sync goes
            // right behind. A pairing syncs nothing.
            let answer = proving.prove(&seed, kind == Kind::Phone).await;
            verdict(kind, proving.name(), &mac, answer)
        })
    }
}

/// What the phone makes of `name`'s answer to its proof, where `mac` is the
/// Mac whose book lists it.
fn verdict(kind: Kind, name: &str, mac: &str, answer: Result<ActionOutcome, LinkError>) -> Verdict {
    match answer {
        Ok(ActionOutcome::Done) => Verdict::Proven,
        Ok(refused) if kind == Kind::Pair => Verdict::Refused(format!(
            "{name} didn't take this pairing code, so scan a new one: {}",
            reason_of(refused)
        )),
        Ok(ActionOutcome::Refused { reason }) if reason.starts_with(UNKNOWN_KEY) => {
            Verdict::Refused(no_longer_known(name, mac))
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

/// The host key the books pin for the device on `link`.
pub fn host_key(books: &[PhoneBook], link: LinkId) -> Option<&str> {
    listed(books)
        .into_iter()
        .find(|(_, entry)| book_link(entry.device) == link)
        .map(|(_, entry)| entry.host_key.as_str())
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
            mac: None,
        }
    }

    /// The test phone's book from the Mac `name`, which lists itself first.
    pub(crate) fn mac_book(name: &str, devices: Vec<PhoneBookEntry>) -> PhoneBook {
        let mut all = vec![entry(name, &[&format!("{name}.local")])];
        all.extend(devices);
        PhoneBook {
            mac: Some(maxhq_protocol::phones::PhoneMac {
                device: DeviceId::derive(name),
                name: name.into(),
            }),
            ..book(all)
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
        let books = [book(vec![entry("devbox", &["devbox.local"])])];
        assert_eq!(
            host_key(&books, book_link(devbox)),
            Some("ssh-ed25519 AAAAexample")
        );
        assert_eq!(host_key(&books, book_link(DeviceId::derive("other"))), None);
    }

    #[test]
    fn each_device_comes_once_under_the_first_mac_that_lists_it() {
        let home = mac_book("Home Mac", vec![entry("devbox", &["192.168.1.20"])]);
        let work = mac_book(
            "Work Mac",
            vec![
                entry("devbox", &["devbox.work"]),
                entry("build-box", &["192.168.1.20"]),
            ],
        );
        let books = [home, work];
        let found: Vec<(usize, &str)> = listed(&books)
            .into_iter()
            .map(|(at, entry)| (at, entry.name.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (0, "Home Mac"),
                (0, "devbox"),
                (1, "Work Mac"),
                (1, "build-box")
            ]
        );
        let macs = MacNames::default();
        macs.keep(&books);
        assert_eq!(macs.of(DeviceId::derive("devbox")), "Home Mac");
        assert_eq!(macs.of(DeviceId::derive("build-box")), "Work Mac");
        assert_eq!(macs.of(DeviceId::derive("other")), "the Mac");
    }

    #[test]
    fn a_book_takes_the_place_of_the_one_from_its_mac() {
        let mut books = Vec::new();
        put_book(&mut books, mac_book("Home Mac", Vec::new()));
        put_book(&mut books, mac_book("Work Mac", Vec::new()));
        let again = mac_book("Home Mac", vec![entry("devbox", &["devbox.local"])]);
        put_book(&mut books, again.clone());
        assert_eq!(books.len(), 2);
        assert_eq!(books[0], again);

        // The same Mac paired again through another keeper.
        let through = PhoneBook {
            device: DeviceId::derive("devbox"),
            ..again
        };
        put_book(&mut books, through.clone());
        assert_eq!(books.len(), 2);
        assert_eq!(books[0], through);
        assert_eq!(mac_name(&books[1]), "Work Mac");
    }

    #[test]
    fn a_book_from_an_older_mac_is_named_by_its_keeper() {
        let older = book(vec![entry("Home Mac", &["home.local"])]);
        assert_eq!(mac_name(&older), "Home Mac");
        let keeperless = PhoneBook {
            device: DeviceId::derive("elsewhere"),
            ..older
        };
        assert_eq!(mac_name(&keeperless), "the Mac");
    }

    #[test]
    fn a_device_that_turns_the_phone_down_says_to_pair_again() {
        let refused = |reason: &str| {
            Ok(ActionOutcome::Refused {
                reason: reason.into(),
            })
        };
        assert_eq!(
            verdict(Kind::Phone, "devbox", "Home Mac", Ok(ActionOutcome::Done)),
            Verdict::Proven
        );
        assert_eq!(
            verdict(
                Kind::Phone,
                "devbox",
                "Home Mac",
                refused("Unknown key: devbox doesn't know this phone's key.")
            ),
            Verdict::Refused(
                "devbox no longer knows this phone. Pair it again from Home Mac.".into()
            )
        );
        // Any other refusal shows the device's reason, without the prefix
        // maxd sorts it by.
        assert_eq!(
            verdict(
                Kind::Phone,
                "devbox",
                "Home Mac",
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
                "the Mac",
                refused("Unknown key: that isn't this pairing's key.")
            ),
            Verdict::Refused(
                "the device didn't take this pairing code, so scan a new one: that isn't this \
                 pairing's key."
                    .into()
            )
        );
        assert_eq!(
            verdict(
                Kind::Phone,
                "devbox",
                "Home Mac",
                Err(LinkError::NotConnected)
            ),
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
