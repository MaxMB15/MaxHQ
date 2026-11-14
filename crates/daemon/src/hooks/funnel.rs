// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The three ways a device's webhooks are reached, one for each
//! [`HookAddress`]: from this device only, over Tailscale Funnel, which
//! `maxd` turns on for its own paths and no others, or under your own
//! address, which `maxd` can't check.

use std::future::ready;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_client::{tailscale_funnel_needs, TailscaleRun};
use maxhq_protocol::tasks::{FunnelState, FunnelStep, HookAddress};
use url::{Host, Url};

use super::Funnel;

/// How long a run of the Tailscale CLI gets. Turning Funnel on can wait
/// for a certificate.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

/// The HTTPS port Funnel serves hooks on.
pub const FUNNEL_PORT: &str = "--https=443";

/// Makes the [`Funnel`] for each address, given the hooks port.
pub type Funnels = Box<dyn Fn(&HookAddress) -> Arc<dyn Funnel> + Send + Sync>;

/// The real [`Funnels`] for the listener on `port`.
pub fn funnels(port: u16) -> Funnels {
    let cli: Arc<dyn Cli> = Arc::new(SystemCli);
    Box::new(move |address| make(address, port, cli.clone()))
}

/// The [`Funnel`] for `address`, in front of the listener on `port`.
pub fn make(address: &HookAddress, port: u16, cli: Arc<dyn Cli>) -> Arc<dyn Funnel> {
    match address {
        HookAddress::Local => Arc::new(Fixed(local_base(port))),
        HookAddress::Funnel => Arc::new(Tailscale {
            port,
            cli,
            failed: Mutex::new(None),
        }),
        HookAddress::Own { base } => Arc::new(Fixed(base.clone())),
    }
}

/// The base of hook URLs on this device alone.
fn local_base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Checks an address you typed and gives it without its trailing slash. It
/// must be https, or http to this device itself.
pub fn own_base(text: &str) -> Result<String, String> {
    let url = Url::parse(text.trim()).map_err(|_| "That isn't a URL.".to_owned())?;
    let local = match url.host() {
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => return Err("That URL has no host.".to_owned()),
    };
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return Err("Use an https address. Plain http works only for localhost.".to_owned()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Leave the user name and password out of the address.".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("Leave the query and the # part out of the address.".to_owned());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// An address `maxd` doesn't change or check: this device's own, or one
/// you point at the hooks port yourself.
struct Fixed(String);

impl Funnel for Fixed {
    fn open(&self, _: &str) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(ready(Ok(())))
    }

    fn close(&self, _: &str) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(ready(Ok(())))
    }

    fn state(&self) -> BoxFuture<'_, FunnelState> {
        Box::pin(ready(FunnelState::On {
            url: self.0.clone(),
        }))
    }

    fn base(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(ready(Ok(self.0.clone())))
    }
}

/// Runs the Tailscale CLI. Tests use a fake.
pub trait Cli: Send + Sync {
    /// Runs `tailscale <args>`.
    fn run(&self, args: Vec<String>) -> BoxFuture<'_, Result<TailscaleRun, String>>;
}

/// The CLI on this machine.
struct SystemCli;

impl Cli for SystemCli {
    fn run(&self, args: Vec<String>) -> BoxFuture<'_, Result<TailscaleRun, String>> {
        Box::pin(async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            maxhq_client::run_tailscale(&args, CLI_TIMEOUT).await
        })
    }
}

/// Tailscale Funnel, which `maxd` turns on and off for each hook's path
/// with `tailscale funnel --set-path`, so the rest of the device's serve
/// and funnel config stays as it was.
struct Tailscale {
    port: u16,
    cli: Arc<dyn Cli>,
    /// What went wrong when a path last wouldn't open, until one does.
    failed: Mutex<Option<FunnelStep>>,
}

impl Tailscale {
    /// Runs `funnel` with `args`, and says what went wrong in words.
    async fn funnel(&self, args: Vec<String>) -> Result<(), FunnelStep> {
        let run = self.cli.run(args).await.map_err(|reason| FunnelStep {
            reason,
            command: None,
        })?;
        if run.ok {
            Ok(())
        } else {
            Err(trouble(&run.stderr, cfg!(target_os = "linux")))
        }
    }

    fn set_failed(&self, step: Option<FunnelStep>) {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = step;
    }

    /// What `tailscale status` says Funnel still needs, and this machine's
    /// name.
    async fn needs(&self) -> (Option<String>, Vec<FunnelStep>) {
        let step = |reason: &str| FunnelStep {
            reason: reason.to_owned(),
            command: None,
        };
        let run = match self.cli.run(args(&["status", "--json"])).await {
            Ok(run) => run,
            Err(reason) => return (None, vec![step(&reason)]),
        };
        let Some(needs) = tailscale_funnel_needs(&run.stdout) else {
            return (
                None,
                vec![step("Tailscale isn't running or signed in on this device.")],
            );
        };
        let mut steps = Vec::new();
        if !needs.https {
            steps.push(step(
                "Turn on HTTPS certificates for the tailnet, in the Tailscale admin console's DNS page.",
            ));
        }
        if !needs.funnel {
            steps.push(step(
                "Add the funnel node attribute for this device to the tailnet policy file, in the Tailscale admin console.",
            ));
        }
        if needs.name.is_none() {
            steps.push(step("This device has no Tailscale name yet."));
        }
        (needs.name, steps)
    }
}

impl Funnel for Tailscale {
    fn open(&self, path: &str) -> BoxFuture<'_, Result<(), String>> {
        let target = local_base(self.port);
        let set_path = format!("--set-path={path}");
        Box::pin(async move {
            let opened = self
                .funnel(args(&["funnel", "--bg", FUNNEL_PORT, &set_path, &target]))
                .await;
            let reason = opened.as_ref().err().map(|step| step.reason.clone());
            self.set_failed(opened.err());
            reason.map_or(Ok(()), Err)
        })
    }

    fn close(&self, path: &str) -> BoxFuture<'_, Result<(), String>> {
        let set_path = format!("--set-path={path}");
        Box::pin(async move {
            self.funnel(args(&["funnel", FUNNEL_PORT, &set_path, "off"]))
                .await
                .map_err(|step| step.reason)
        })
    }

    fn state(&self) -> BoxFuture<'_, FunnelState> {
        Box::pin(async move {
            let (name, mut steps) = self.needs().await;
            let failed = self
                .failed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            steps.extend(failed);
            match name {
                Some(name) if steps.is_empty() => FunnelState::On {
                    url: format!("https://{name}"),
                },
                _ => FunnelState::NeedsSetup { steps },
            }
        })
    }

    fn base(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            match self.needs().await {
                (Some(name), _) => Ok(format!("https://{name}")),
                (None, _) => Err("This device has no Tailscale name yet.".to_owned()),
            }
        })
    }
}

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// What a failed `tailscale funnel` means, from what it printed. On Linux,
/// only root or the operator may change Funnel.
fn trouble(stderr: &str, linux: bool) -> FunnelStep {
    if linux && stderr.to_ascii_lowercase().contains("access denied") {
        let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_owned());
        return FunnelStep {
            reason: "Let maxd's user change Tailscale. Run this once on this device.".to_owned(),
            command: Some(format!("sudo tailscale set --operator={user}")),
        };
    }
    let said = stderr.lines().next().unwrap_or("").trim();
    FunnelStep {
        reason: if said.is_empty() {
            "Tailscale couldn't turn Funnel on.".to_owned()
        } else {
            format!("Tailscale couldn't turn Funnel on: {said}")
        },
        command: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps each command and answers from a script: `status --json` with
    /// `status`, and `funnel` with `funnel`.
    struct FakeCli {
        calls: Mutex<Vec<String>>,
        status: Result<TailscaleRun, String>,
        funnel: TailscaleRun,
    }

    impl FakeCli {
        fn new(status: &str) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::default(),
                status: Ok(TailscaleRun {
                    ok: true,
                    stdout: status.as_bytes().to_vec(),
                    stderr: String::new(),
                }),
                funnel: TailscaleRun {
                    ok: true,
                    ..TailscaleRun::default()
                },
            })
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Cli for FakeCli {
        fn run(&self, args: Vec<String>) -> BoxFuture<'_, Result<TailscaleRun, String>> {
            self.calls.lock().unwrap().push(args.join(" "));
            let answer = if args[0] == "status" {
                self.status.clone()
            } else {
                Ok(self.funnel.clone())
            };
            Box::pin(ready(answer))
        }
    }

    const READY: &str = r#"{
        "CertDomains": ["devbox.example.ts.net"],
        "Self": {"DNSName": "devbox.example.ts.net.", "CapMap": {"funnel": null, "https": null}}
    }"#;

    fn tailscale(cli: Arc<FakeCli>) -> Arc<dyn Funnel> {
        make(&HookAddress::Funnel, 41234, cli)
    }

    fn reasons(state: FunnelState) -> Vec<String> {
        let FunnelState::NeedsSetup { steps } = state else {
            panic!("not needs setup: {state:?}");
        };
        steps.into_iter().map(|step| step.reason).collect()
    }

    #[tokio::test]
    async fn this_device_only_uses_the_local_url_and_changes_nothing() {
        let funnel = make(&HookAddress::Local, 41234, FakeCli::new(READY));
        assert_eq!(funnel.open("/maxhq/t").await, Ok(()));
        assert_eq!(funnel.base().await.as_deref(), Ok("http://127.0.0.1:41234"));
        assert_eq!(
            funnel.state().await,
            FunnelState::On {
                url: "http://127.0.0.1:41234".into()
            }
        );
    }

    #[tokio::test]
    async fn your_own_address_is_the_base_and_changes_nothing() {
        let cli = FakeCli::new(READY);
        let own = HookAddress::Own {
            base: "https://hooks.example.com/in".into(),
        };
        let funnel = make(&own, 41234, cli.clone());
        assert_eq!(funnel.close("/maxhq/t").await, Ok(()));
        assert_eq!(
            funnel.base().await.as_deref(),
            Ok("https://hooks.example.com/in")
        );
        assert!(cli.calls().is_empty());
    }

    #[tokio::test]
    async fn funnel_adds_and_removes_only_its_own_path() {
        let cli = FakeCli::new(READY);
        let funnel = tailscale(cli.clone());
        assert_eq!(funnel.open("/maxhq/t1").await, Ok(()));
        assert_eq!(funnel.close("/maxhq/t1").await, Ok(()));
        assert_eq!(
            cli.calls(),
            [
                "funnel --bg --https=443 --set-path=/maxhq/t1 http://127.0.0.1:41234",
                "funnel --https=443 --set-path=/maxhq/t1 off",
            ]
        );
        assert_eq!(
            funnel.state().await,
            FunnelState::On {
                url: "https://devbox.example.ts.net".into()
            }
        );
        assert_eq!(
            funnel.base().await.as_deref(),
            Ok("https://devbox.example.ts.net")
        );
    }

    #[tokio::test]
    async fn funnel_says_what_the_tailnet_is_missing() {
        let funnel = tailscale(FakeCli::new(
            r#"{"Self": {"DNSName": "devbox.example.ts.net."}}"#,
        ));
        let missing = reasons(funnel.state().await);
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(missing[0].contains("HTTPS certificates"));
        assert!(missing[1].contains("funnel node attribute"));

        let stopped = tailscale(FakeCli::new("{}"));
        assert_eq!(
            reasons(stopped.state().await),
            ["Tailscale isn't running or signed in on this device."]
        );
        let nameless = tailscale(FakeCli::new("{}"));
        assert_eq!(
            nameless.base().await,
            Err("This device has no Tailscale name yet.".into())
        );
    }

    #[tokio::test]
    async fn a_path_that_wont_open_shows_until_one_does() {
        let cli = Arc::new(FakeCli {
            funnel: TailscaleRun {
                ok: false,
                stdout: Vec::new(),
                stderr: "error: serve config denied\nmore".into(),
            },
            ..Arc::into_inner(FakeCli::new(READY)).unwrap()
        });
        let funnel = tailscale(cli);
        let reason = funnel.open("/maxhq/t").await.unwrap_err();
        assert_eq!(
            reason,
            "Tailscale couldn't turn Funnel on: error: serve config denied"
        );
        assert_eq!(reasons(funnel.state().await), [reason]);
    }

    #[test]
    fn access_denied_on_linux_asks_for_the_operator() {
        let step = trouble("Access denied: serve config denied", true);
        assert!(step
            .command
            .unwrap()
            .starts_with("sudo tailscale set --operator="));
        assert_eq!(
            trouble("Access denied", false).command,
            None,
            "only Linux has an operator"
        );
    }

    #[test]
    fn your_own_address_must_be_https_or_local() {
        assert_eq!(
            own_base(" https://hooks.example.com/ "),
            Ok("https://hooks.example.com".into())
        );
        assert_eq!(
            own_base("http://localhost:8080/x/"),
            Ok("http://localhost:8080/x".into())
        );
        assert!(own_base("http://127.0.0.1:9").is_ok());
        assert!(own_base("http://[::1]:9").is_ok());
        for bad in [
            "http://hooks.example.com",
            "ftp://hooks.example.com",
            "hooks.example.com",
            "https://me:pw@hooks.example.com",
            "https://hooks.example.com/?a=1",
            "https://hooks.example.com/#a",
        ] {
            assert!(own_base(bad).is_err(), "{bad}");
        }
    }
}
