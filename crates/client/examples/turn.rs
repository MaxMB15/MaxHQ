// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Starts one agent on a device, prints its timeline until the turn ends,
//! then stops it, for trying a driver live without the app.
//!
//! `cargo run -p maxhq-client --example turn -- local claude /tmp/scratch
//! "Say hi" --model haiku --permission full_access` runs one Claude turn on
//! this machine. The link reconnects on its own, so restarting `maxd`
//! mid-turn shows whether the turn carries on. `--keep` leaves the agent
//! running instead of stopping it. The stop tries again while the link
//! reconnects, so a restart just before it still stops the agent.

use std::time::Duration;

use maxhq_client::{Client, LinkError, LinkEvent, LinkHandle, LinkSpec};
use maxhq_protocol::ids::{AccountId, AgentId, DeviceId, IdempotencyKey, LinkId};
use maxhq_protocol::link::{LinkState, Route};
use maxhq_protocol::message::{ActionOutcome, AgentPlace, AgentSpec, ClientMessage, UserInput};
use maxhq_protocol::model::{Activity, PermissionMode, Vendor};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::timeline::{TimelineEvent, TimelineItem, TimelineUpdate};
use tokio::sync::mpsc;

/// How long one turn may take before the example gives up.
const TURN_LIMIT: Duration = Duration::from_secs(300);

/// How long to keep printing the timeline after the turn ends.
const TRAIL: Duration = Duration::from_secs(1);

/// How long the stop keeps trying while the link reconnects.
const STOP_LIMIT: Duration = Duration::from_secs(30);

struct Args {
    host: String,
    vendor: Vendor,
    folder: String,
    prompt: String,
    model: Option<String>,
    permission: Option<PermissionMode>,
    keep: bool,
}

#[tokio::main]
async fn main() {
    let args = parse_args().unwrap_or_else(|problem| {
        eprintln!("{problem}");
        eprintln!(
            "usage: turn local|<ssh host> claude|codex <folder> <prompt> \
             [--model M] [--permission ask|accept_edits|auto|plan|full_access] [--keep]"
        );
        std::process::exit(2);
    });
    let route = if args.host == "local" {
        Route::Local
    } else {
        Route::Ssh {
            host: args.host.clone(),
        }
    };
    let client = Client::new();
    let (link, mut events) = client.link(LinkSpec {
        id: LinkId::derive(&args.host),
        name: args.host.clone(),
        route,
        fallbacks: Vec::new(),
        last_good: Default::default(),
    });

    let account = match tokio::time::timeout(
        Duration::from_secs(60),
        find_account(&mut events, args.vendor),
    )
    .await
    {
        Ok(Some(account)) => account,
        Ok(None) => fail("the link closed before it synced"),
        Err(_) => fail("no account for that vendor on the device within a minute"),
    };

    let spec = AgentSpec {
        vendor: args.vendor,
        account,
        place: AgentPlace::Folder {
            path: args.folder.clone(),
        },
        model: args.model.clone(),
        effort: None,
        permission: args.permission,
        title: Some("turn example".into()),
        first: Some(UserInput {
            text: args.prompt.clone(),
            attachments: Vec::new(),
            refs: Vec::new(),
        }),
        branch: None,
        origin: None,
    };
    let agent = match act(
        &link,
        ClientMessage::StartAgent {
            key: IdempotencyKey::generate(),
            spec,
        },
    )
    .await
    {
        ActionOutcome::Started { agent } => agent,
        other => fail(&format!("start: {other:?}")),
    };
    println!("started {agent}");
    if let Err(error) = link.open_timeline(agent, None, 200).await {
        fail(&format!("open timeline: {error}"));
    }

    let ended = tokio::time::timeout(TURN_LIMIT, follow(&mut events, agent)).await;
    match ended {
        Ok(Some(activity)) => println!("turn ended: {activity:?}"),
        Ok(None) => println!("the link closed"),
        Err(_) => println!("gave up after {}s", TURN_LIMIT.as_secs()),
    }
    // The timeline comes apart from the agent's activity, so its last items
    // can land just after the turn ends.
    let _ = tokio::time::timeout(TRAIL, trail(&mut events, agent)).await;
    // Nothing reads the events from here on, so drain them to keep the link
    // free to reconnect.
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    if args.keep {
        println!("kept {agent} running");
    } else {
        println!("stop: {:?}", stop(&link, agent).await);
    }
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut model = None;
    let mut permission = None;
    let mut keep = false;
    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--model" => model = Some(rest.next().ok_or("--model needs a value")?),
            "--permission" => {
                let value = rest.next().ok_or("--permission needs a value")?;
                permission = Some(
                    serde_json::from_value(serde_json::Value::String(value.clone()))
                        .map_err(|_| format!("no permission mode called {value}"))?,
                );
            }
            "--keep" => keep = true,
            _ => positional.push(arg),
        }
    }
    let [host, vendor, folder, prompt] =
        <[String; 4]>::try_from(positional).map_err(|_| "expected four arguments".to_owned())?;
    let vendor = match vendor.as_str() {
        "claude" => Vendor::Claude,
        "codex" => Vendor::Codex,
        other => return Err(format!("no vendor called {other}")),
    };
    Ok(Args {
        host,
        vendor,
        folder,
        prompt,
        model,
        permission,
        keep,
    })
}

/// Waits for the device to sync, then picks its first login for the vendor.
/// Logins inside containers the device watches belong to other devices, so
/// they're skipped.
async fn find_account(events: &mut mpsc::Receiver<LinkEvent>, vendor: Vendor) -> Option<AccountId> {
    let mut device: Option<DeviceId> = None;
    while let Some(event) = events.recv().await {
        match event {
            LinkEvent::Link(link) => {
                println!("link {}", state_name(&link.state));
                device = link.device.or(device);
            }
            LinkEvent::Synced(entities) => {
                let found = entities.iter().find_map(|entity| match entity {
                    Entity::Account(account)
                        if account.vendor == vendor && Some(account.device) == device =>
                    {
                        Some(account)
                    }
                    _ => None,
                });
                if let Some(account) = found {
                    println!("account {} plan={:?}", account.label, account.plan);
                    return Some(account.id);
                }
            }
            _ => {}
        }
    }
    None
}

/// Prints the agent's timeline and activity until its turn ends, and hands
/// back the activity it ended on.
async fn follow(events: &mut mpsc::Receiver<LinkEvent>, agent: AgentId) -> Option<Activity> {
    let mut worked = false;
    let mut appended = 0usize;
    while let Some(event) = events.recv().await {
        match event {
            LinkEvent::Link(link) => println!("link {}", state_name(&link.state)),
            LinkEvent::Synced(entities) => {
                println!("synced again");
                for entity in entities {
                    if let Some(done) = activity(&entity, agent, &mut worked) {
                        return Some(done);
                    }
                }
            }
            LinkEvent::Changed(changes) => {
                for change in changes {
                    if let Change::Upsert { value } = change
                        && let Some(done) = activity(&value, agent, &mut worked)
                    {
                        return Some(done);
                    }
                }
            }
            LinkEvent::Timeline {
                agent: from,
                update,
                ..
            } if from == agent => print_update(&update, &mut appended),
            LinkEvent::Timeline { .. }
            | LinkEvent::Workflow { .. }
            | LinkEvent::RouteWorked { .. }
            | LinkEvent::PeerRequest { .. } => {}
        }
    }
    None
}

/// Prints the agent's timeline until the link closes.
async fn trail(events: &mut mpsc::Receiver<LinkEvent>, agent: AgentId) {
    let mut appended = 0usize;
    while let Some(event) = events.recv().await {
        if let LinkEvent::Timeline {
            agent: from,
            update,
            ..
        } = event
            && from == agent
        {
            print_update(&update, &mut appended);
        }
    }
}

/// Prints the agent's activity when it changes, and hands it back once a
/// turn that started has ended.
fn activity(entity: &Entity, agent: AgentId, worked: &mut bool) -> Option<Activity> {
    let Entity::Agent(found) = entity else {
        return None;
    };
    if found.id != agent {
        return None;
    }
    println!("activity {:?}", found.activity);
    match &found.activity {
        Activity::Busy | Activity::BusyInSubagents => {
            *worked = true;
            None
        }
        Activity::Starting | Activity::NeedsInput { .. } => None,
        Activity::DoneUnread | Activity::Idle | Activity::IdleCacheWarm { .. } if *worked => {
            Some(found.activity.clone())
        }
        Activity::Paused { .. } | Activity::Error { .. } => Some(found.activity.clone()),
        _ => None,
    }
}

fn print_update(update: &TimelineUpdate, appended: &mut usize) {
    match update {
        TimelineUpdate::Reset { seq, items, .. } => {
            println!("reset at {seq:?} with {} items", items.len());
            for item in items {
                print_item(item);
            }
        }
        TimelineUpdate::Events { seq, events } => {
            for event in events {
                match event {
                    TimelineEvent::Upsert { item } => {
                        if *appended > 0 {
                            println!("  +{appended} chars streamed");
                            *appended = 0;
                        }
                        print_item(item);
                    }
                    TimelineEvent::Append { text, .. } => *appended += text.chars().count(),
                    TimelineEvent::Remove { id } => println!("  removed {id:?} at {seq:?}"),
                }
            }
        }
        TimelineUpdate::Older { items, .. } => println!("{} older items", items.len()),
    }
}

/// One line per item: its kind, status and the start of its text.
fn print_item(item: &TimelineItem) {
    let body = serde_json::to_value(&item.body).unwrap_or_default();
    let kind = body["kind"].as_str().unwrap_or("?");
    let text = ["text", "command", "title", "path"]
        .iter()
        .find_map(|field| body[field].as_str())
        .unwrap_or("");
    let short: String = text.chars().take(80).collect();
    println!(
        "  #{} {kind} {:?} {}",
        item.ordinal,
        item.status,
        short.replace('\n', " ")
    );
}

/// Stops the agent, sending the same key again while the link is down.
async fn stop(link: &LinkHandle, agent: AgentId) -> ActionOutcome {
    let key = IdempotencyKey::generate();
    let deadline = tokio::time::Instant::now() + STOP_LIMIT;
    loop {
        match link.act(ClientMessage::StopAgent { key, agent }).await {
            Ok(outcome) => return outcome,
            Err(LinkError::NotConnected) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => fail(&format!("stop failed: {error}")),
        }
    }
}

async fn act(link: &LinkHandle, message: ClientMessage) -> ActionOutcome {
    match link.act(message).await {
        Ok(outcome) => outcome,
        Err(error) => fail(&format!("action failed: {error}")),
    }
}

fn state_name(state: &LinkState) -> String {
    let json = serde_json::to_value(state).unwrap_or_default();
    json["state"].as_str().unwrap_or("?").to_owned()
}

fn fail(problem: &str) -> ! {
    eprintln!("{problem}");
    std::process::exit(1);
}
