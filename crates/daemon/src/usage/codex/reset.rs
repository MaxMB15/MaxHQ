// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Spends one of a Codex account's banked rate-limit resets, which can't be
//! undone. Only the app asks, for an explicit click after you confirm, and
//! the server refuses everyone else first. A short-lived app-server for the
//! account's `CODEX_HOME` sends the request with Codex's own sign-in, then
//! reads the limits again so the reset shows at once. A reset that landed
//! is passed on, so the agents its limit held can carry on.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use maxhq_protocol::ids::{AccountId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, ResetCreditOutcome};
use maxhq_protocol::model::{LoginKind, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};

use super::account::find_codex;
use super::app_server::{AppServer, Error, Method};
use super::limits;
use crate::hub::{Command, Hub};

/// The refusal while a reset for the same account is still out.
const BUSY: &str = "A reset for this account is already in progress. Wait for it to finish.";

/// The accounts a reset is being spent for now.
static SPENDING: LazyLock<Mutex<HashSet<AccountId>>> = LazyLock::new(Mutex::default);

/// Holds an account's place in [`SPENDING`] until dropped.
struct Spending(AccountId);

impl Spending {
    /// Takes the account's place, or None while another spend holds it.
    fn begin(account: AccountId) -> Option<Self> {
        let mut spending = SPENDING.lock().unwrap_or_else(PoisonError::into_inner);
        // Built only once the place is taken: a Spending dropped here would
        // free the place of the spend that holds it.
        spending.insert(account).then(|| Self(account))
    }
}

impl Drop for Spending {
    fn drop(&mut self) {
        let mut spending = SPENDING.lock().unwrap_or_else(PoisonError::into_inner);
        spending.remove(&self.0);
    }
}

/// Spends one of `account`'s banked resets, with `key` as Codex's
/// idempotency key, so a resend of the same request can't spend a second.
/// `lifted` hears of a reset that landed, once the limits are read again,
/// and of nothing else.
pub async fn use_credit(
    hub: Hub,
    key: IdempotencyKey,
    account: AccountId,
    lifted: impl FnOnce(AccountId),
) -> ActionOutcome {
    use_credit_with(hub, key, account, find_codex, lifted).await
}

/// [`use_credit`] with the way to find `codex` given, for tests.
pub(crate) async fn use_credit_with(
    hub: Hub,
    key: IdempotencyKey,
    account: AccountId,
    program: impl Fn() -> Option<PathBuf>,
    lifted: impl FnOnce(AccountId),
) -> ActionOutcome {
    let home = match home(&hub, account) {
        Ok(home) => home,
        Err(reason) => return refused(reason),
    };
    let Some(_spending) = Spending::begin(account) else {
        return refused(BUSY);
    };
    let Some(program) = program() else {
        return refused("Couldn't find codex on this device.");
    };
    let (mut server, answer) = match spend(&program, &home, key).await {
        Ok(spent) => spent,
        Err(Unsure::NotSent(error)) => {
            return refused(&format!("Couldn't use a reset: {error}."));
        }
        Err(Unsure::Unanswered(error)) => {
            tracing::warn!(%account, %error, "codex didn't answer a reset twice");
            hub.send(Command::RefreshUsage(Some(account)));
            return refused(
                "Codex didn't answer, so maxd can't tell whether a reset was used. Check the \
                 count before trying again.",
            );
        }
    };
    // Every answer rereads the limits: a reset changes them, and a count
    // that was out of date shows right once Codex says none are left.
    reread(&hub, account, &mut server).await;
    server.stop().await;
    match outcome(&answer) {
        Some(outcome) => {
            tracing::info!(%account, ?outcome, "codex reset credit");
            if landed(outcome) {
                lifted(account);
            }
            ActionOutcome::ResetCredit { outcome }
        }
        None => refused(&format!(
            "Codex answered {answer}, which this maxd doesn't know. Check the count before \
             trying again."
        )),
    }
}

/// The account's `CODEX_HOME`, or why its resets can't be spent here.
fn home(hub: &Hub, account: AccountId) -> Result<PathBuf, &'static str> {
    let Some(Entity::Account(account)) = hub.get(&EntityKey::Account(account)) else {
        return Err("maxd doesn't know that account.");
    };
    if account.vendor != Vendor::Codex {
        return Err("Only a Codex account has resets to use.");
    }
    if account.device != hub.device().id {
        return Err("That account is on another device, whose own maxd uses its resets.");
    }
    if account.login != LoginKind::ConfigDir {
        return Err(
            "Use reset works only for a Codex login kept in its own folder on this device.",
        );
    }
    Ok(PathBuf::from(account.config_dir))
}

/// Why a spend has no answer.
enum Unsure {
    /// It failed before the request went out, or Codex answered with an
    /// error, so nothing was spent.
    NotSent(Error),
    /// The request went out twice and neither try came back, so a reset
    /// may or may not be spent.
    Unanswered(Error),
}

/// Sends the spend and returns its answer with the app-server still open.
/// A try whose answer never comes back, because the app-server exited or
/// took too long, goes once more through a new app-server with the same
/// key, which Codex answers without spending twice.
async fn spend(
    program: &Path,
    home: &Path,
    key: IdempotencyKey,
) -> Result<(AppServer, Value), Unsure> {
    let params = json!({ "idempotencyKey": key.to_string() });
    let mut sent = false;
    loop {
        let mut server = match AppServer::start(program, home).await {
            Ok(server) => server,
            Err(error) if sent => return Err(Unsure::Unanswered(error)),
            Err(error) => return Err(Unsure::NotSent(error)),
        };
        match server.call(Method::UseResetCredit, params.clone()).await {
            Ok(answer) => return Ok((server, answer)),
            Err(error) => {
                server.stop().await;
                if !error.needs_restart() {
                    return Err(Unsure::NotSent(error));
                }
                if sent {
                    return Err(Unsure::Unanswered(error));
                }
                tracing::debug!(%error, "codex reset went unanswered, so it goes again");
                sent = true;
            }
        }
    }
}

/// What Codex's answer came to, or None for an outcome this `maxd`
/// doesn't know.
fn outcome(answer: &Value) -> Option<ResetCreditOutcome> {
    match answer.get("outcome")?.as_str()? {
        "reset" => Some(ResetCreditOutcome::Reset),
        "nothingToReset" => Some(ResetCreditOutcome::NothingToReset),
        "noCredit" => Some(ResetCreditOutcome::NoCredit),
        "alreadyRedeemed" => Some(ResetCreditOutcome::AlreadyRedeemed),
        _ => None,
    }
}

/// Whether the limit is lifted after `outcome`: a reset spent now, or one
/// an earlier try with the same key spent. Nothing to reset and no credit
/// left change nothing.
fn landed(outcome: ResetCreditOutcome) -> bool {
    match outcome {
        ResetCreditOutcome::Reset | ResetCreditOutcome::AlreadyRedeemed => true,
        ResetCreditOutcome::NothingToReset | ResetCreditOutcome::NoCredit => false,
    }
}

/// Reads the limits again and shows them. When that fails, the account's
/// next check does it.
async fn reread(hub: &Hub, account: AccountId, server: &mut AppServer) {
    let params = json!({ "excludeResetCreditDetails": true });
    match server.call(Method::ReadRateLimits, params).await {
        Ok(result) => {
            let reading = limits::from_app_server(&result, Timestamp::now());
            let shown = match hub.get(&EntityKey::Account(account)) {
                Some(Entity::Account(account)) => account.usage,
                _ => None,
            };
            if let Some(usage) = limits::merge(shown, &reading) {
                hub.set_usage(account, usage);
            }
        }
        Err(error) => {
            tracing::debug!(%account, %error, "couldn't read the limits after a reset");
            hub.send(Command::RefreshUsage(Some(account)));
        }
    }
}

/// A refusal with `reason`.
fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app_server::tests::FakeHome;
    use super::*;
    use maxhq_protocol::model::Account;
    use maxhq_protocol::state::Change;

    /// A hub that knows a Codex account whose `CODEX_HOME` is the fake's.
    fn setup(name: &str) -> (Hub, FakeHome, AccountId) {
        let fake = FakeHome::new(name);
        let hub = crate::secrets::testing::hub();
        let id = AccountId::derive(name);
        hub.apply([Change::Upsert {
            value: Entity::Account(Account {
                id,
                device: hub.device().id,
                vendor: Vendor::Codex,
                label: "Codex".into(),
                config_dir: fake.home.to_string_lossy().into_owned(),
                email: None,
                organization: None,
                plan: None,
                models: Vec::new(),
                usage: None,
                usage_failure: None,
                acp: None,
                login: LoginKind::ConfigDir,
                origin: None,
                model_server: None,
            }),
        }]);
        (hub, fake, id)
    }

    fn methods(fake: &FakeHome) -> Vec<String> {
        fake.requests()
            .iter()
            .map(|request| request["method"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn reset_credits(hub: &Hub, account: AccountId) -> Option<u32> {
        match hub.get(&EntityKey::Account(account)) {
            Some(Entity::Account(account)) => account.usage?.reset_credits,
            other => panic!("no account: {other:?}"),
        }
    }

    async fn spend(
        hub: &Hub,
        fake: &FakeHome,
        key: IdempotencyKey,
        id: AccountId,
    ) -> ActionOutcome {
        heard(hub, fake, key, id).await.0
    }

    /// A spend, and the accounts it said a reset landed for.
    async fn heard(
        hub: &Hub,
        fake: &FakeHome,
        key: IdempotencyKey,
        id: AccountId,
    ) -> (ActionOutcome, Vec<AccountId>) {
        let program = fake.program.clone();
        let mut lifted = Vec::new();
        let outcome = use_credit_with(
            hub.clone(),
            key,
            id,
            move || Some(program.clone()),
            |account| lifted.push(account),
        )
        .await;
        (outcome, lifted)
    }

    #[tokio::test]
    async fn each_answer_comes_back_as_its_outcome() {
        let (hub, fake, id) = setup("codex-reset-outcomes");
        for (answer, outcome, lifts) in [
            ("reset", ResetCreditOutcome::Reset, true),
            ("nothingToReset", ResetCreditOutcome::NothingToReset, false),
            ("noCredit", ResetCreditOutcome::NoCredit, false),
            ("alreadyRedeemed", ResetCreditOutcome::AlreadyRedeemed, true),
        ] {
            fake.write("consume.json", &json!({ "outcome": answer }));
            let (spent, lifted) = heard(&hub, &fake, IdempotencyKey::generate(), id).await;
            assert_eq!(spent, ActionOutcome::ResetCredit { outcome }, "{answer}");
            let expected = if lifts { vec![id] } else { Vec::new() };
            assert_eq!(
                lifted, expected,
                "{answer} lifts the limit only if it landed"
            );
        }
        fake.write("consume.json", &json!({ "outcome": "somethingNew" }));
        let (outcome, lifted) = heard(&hub, &fake, IdempotencyKey::generate(), id).await;
        assert!(
            matches!(outcome, ActionOutcome::Refused { .. }),
            "{outcome:?}"
        );
        assert!(
            lifted.is_empty(),
            "an answer maxd doesn't know lifts nothing"
        );
    }

    #[tokio::test]
    async fn only_a_reset_that_landed_lifts_the_limit() {
        // Unanswered twice: maxd can't tell, so nothing carries on.
        let (hub, fake, id) = setup("codex-reset-lift-unsure");
        fake.write("consume.json", &json!({ "outcome": "reset" }));
        fake.write("drop-every-consume", &json!(true));
        let (outcome, lifted) = heard(&hub, &fake, IdempotencyKey::generate(), id).await;
        assert!(
            matches!(outcome, ActionOutcome::Refused { .. }),
            "{outcome:?}"
        );
        assert!(lifted.is_empty(), "an unsure spend lifts nothing");
        fake.remove("drop-every-consume");

        // A refusal before anything went out.
        let (outcome, lifted) = heard(
            &hub,
            &fake,
            IdempotencyKey::generate(),
            AccountId::derive("nobody"),
        )
        .await;
        assert!(
            matches!(outcome, ActionOutcome::Refused { .. }),
            "{outcome:?}"
        );
        assert!(lifted.is_empty(), "a refusal lifts nothing");

        // A dropped answer that went again and landed lifts it once.
        fake.write("consume.json", &json!({ "outcome": "alreadyRedeemed" }));
        fake.write("drop-consume", &json!(true));
        let (_, lifted) = heard(&hub, &fake, IdempotencyKey::generate(), id).await;
        assert_eq!(lifted, [id], "one landing, heard once");
    }

    #[tokio::test]
    async fn a_reset_sends_the_key_and_rereads_the_limits() {
        let (hub, fake, id) = setup("codex-reset-reread");
        fake.write("consume.json", &json!({ "outcome": "reset" }));
        let mut limits: Value =
            serde_json::from_str(include_str!("fixtures/rate_limits.json")).unwrap();
        limits["rateLimitResetCredits"]["availableCount"] = json!(1);
        fake.write("limits.json", &limits);
        let key = IdempotencyKey::generate();

        let outcome = spend(&hub, &fake, key, id).await;
        assert_eq!(
            outcome,
            ActionOutcome::ResetCredit {
                outcome: ResetCreditOutcome::Reset
            }
        );
        assert_eq!(
            methods(&fake),
            [
                "initialize",
                "initialized",
                "account/rateLimitResetCredit/consume",
                "account/rateLimits/read"
            ]
        );
        let requests = fake.requests();
        assert_eq!(
            requests[2]["params"],
            json!({ "idempotencyKey": key.to_string() })
        );
        assert_eq!(
            requests[3]["params"],
            json!({ "excludeResetCreditDetails": true })
        );
        assert_eq!(reset_credits(&hub, id), Some(1), "the reread shows at once");
        assert_eq!(fake.stops(), 1, "the app-server stops after");
    }

    #[tokio::test]
    async fn a_dropped_answer_goes_again_with_the_same_key() {
        let (hub, fake, id) = setup("codex-reset-retry");
        fake.write("consume.json", &json!({ "outcome": "alreadyRedeemed" }));
        fake.write("drop-consume", &json!(true));
        let key = IdempotencyKey::generate();

        let outcome = spend(&hub, &fake, key, id).await;
        assert_eq!(
            outcome,
            ActionOutcome::ResetCredit {
                outcome: ResetCreditOutcome::AlreadyRedeemed
            }
        );
        let consumed: Vec<Value> = fake
            .requests()
            .into_iter()
            .filter(|request| request["method"] == "account/rateLimitResetCredit/consume")
            .map(|request| request["params"].clone())
            .collect();
        let same = json!({ "idempotencyKey": key.to_string() });
        assert_eq!(consumed, [same.clone(), same], "two tries, one key");
    }

    #[tokio::test]
    async fn a_second_try_that_drops_too_says_it_cant_tell() {
        let (hub, fake, id) = setup("codex-reset-unsure");
        fake.write("consume.json", &json!({ "outcome": "reset" }));
        fake.write("drop-every-consume", &json!(true));
        let outcome = spend(&hub, &fake, IdempotencyKey::generate(), id).await;
        let ActionOutcome::Refused { reason } = outcome else {
            panic!("expected a refusal, got {outcome:?}");
        };
        assert!(reason.contains("can't tell"), "{reason}");
        let consumed = methods(&fake)
            .iter()
            .filter(|method| method.ends_with("consume"))
            .count();
        assert_eq!(consumed, 2, "one try and one more, never a third");
    }

    #[tokio::test]
    async fn a_second_spend_for_the_account_is_refused_while_one_runs() {
        let (hub, fake, id) = setup("codex-reset-busy");
        fake.write("consume.json", &json!({ "outcome": "reset" }));
        fake.write("hold", &json!(true));
        let first = tokio::spawn({
            let (hub, program) = (hub.clone(), fake.program.clone());
            async move {
                use_credit_with(
                    hub,
                    IdempotencyKey::generate(),
                    id,
                    move || Some(program.clone()),
                    |_| {},
                )
                .await
            }
        });
        // Wait until the first one's request is out.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !methods(&fake)
            .iter()
            .any(|method| method.ends_with("consume"))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the spend never went out"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let second = spend(&hub, &fake, IdempotencyKey::generate(), id).await;
        assert_eq!(second, refused(BUSY));
        fake.remove("hold");
        assert_eq!(
            first.await.unwrap(),
            ActionOutcome::ResetCredit {
                outcome: ResetCreditOutcome::Reset
            }
        );
        let consumed = methods(&fake)
            .iter()
            .filter(|method| method.ends_with("consume"))
            .count();
        assert_eq!(consumed, 1, "the refused one sent nothing");

        // Once it's done, the next one goes.
        let third = spend(&hub, &fake, IdempotencyKey::generate(), id).await;
        assert!(
            matches!(third, ActionOutcome::ResetCredit { .. }),
            "{third:?}"
        );
    }

    #[tokio::test]
    async fn only_a_codex_login_on_this_device_spends() {
        let (hub, fake, id) = setup("codex-reset-whose");
        fake.write("consume.json", &json!({ "outcome": "reset" }));
        let Some(Entity::Account(account)) = hub.get(&EntityKey::Account(id)) else {
            panic!("no account");
        };
        let elsewhere = AccountId::derive("codex-reset-whose-elsewhere");
        let brokered = AccountId::derive("codex-reset-whose-brokered");
        let claude = AccountId::derive("codex-reset-whose-claude");
        hub.apply([
            Change::Upsert {
                value: Entity::Account(Account {
                    id: elsewhere,
                    device: maxhq_protocol::ids::DeviceId::derive("devbox"),
                    ..account.clone()
                }),
            },
            Change::Upsert {
                value: Entity::Account(Account {
                    id: brokered,
                    login: LoginKind::ChatGptSignIn,
                    ..account.clone()
                }),
            },
            Change::Upsert {
                value: Entity::Account(Account {
                    id: claude,
                    vendor: Vendor::Claude,
                    ..account
                }),
            },
        ]);
        for account in [elsewhere, brokered, claude, AccountId::derive("nobody")] {
            let outcome = spend(&hub, &fake, IdempotencyKey::generate(), account).await;
            assert!(
                matches!(outcome, ActionOutcome::Refused { .. }),
                "{outcome:?}"
            );
        }
        assert!(fake.requests().is_empty(), "no app-server started");
    }
}
