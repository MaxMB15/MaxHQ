// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Connects to one or more devices and prints what they report, for
//! checking a `maxd` by hand without the app.
//!
//! `cargo run -p maxhq-client --example watch -- local dev@devbox` links to
//! this machine's `maxd` and to devbox's over SSH. Each link prints its
//! state changes, then every agent and account once it syncs, and one line
//! per later change. It never writes anything.

use std::collections::BTreeMap;

use maxhq_client::{Client, LinkEvent, LinkSpec};
use maxhq_protocol::ids::LinkId;
use maxhq_protocol::link::Route;
use maxhq_protocol::state::{Change, Entity};

#[tokio::main]
async fn main() {
    let hosts: Vec<String> = std::env::args().skip(1).collect();
    if hosts.is_empty() {
        eprintln!("usage: watch local|<ssh host>...");
        std::process::exit(2);
    }
    let client = Client::new();
    let mut tasks = Vec::new();
    for host in hosts {
        let route = if host == "local" {
            Route::Local
        } else {
            Route::Ssh { host: host.clone() }
        };
        let spec = LinkSpec {
            id: LinkId::derive(&host),
            name: host.clone(),
            route,
            fallbacks: Vec::new(),
            last_good: Default::default(),
        };
        let (handle, mut events) = client.link(spec);
        tasks.push(tokio::spawn(async move {
            // Dropping the handle closes the link, so hold it.
            let _handle = handle;
            while let Some(event) = events.recv().await {
                print_event(&host, event);
            }
        }));
    }
    for task in tasks {
        let _ = task.await;
    }
}

fn print_event(host: &str, event: LinkEvent) {
    match event {
        LinkEvent::Link(link) => println!("[{host}] link {:?}", link.state),
        LinkEvent::Synced(entities) => {
            let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            for entity in &entities {
                *kinds.entry(kind(entity)).or_default() += 1;
            }
            println!("[{host}] synced {kinds:?}");
            for entity in &entities {
                print_entity(host, entity);
            }
        }
        LinkEvent::Changed(changes) => {
            for change in changes {
                match change {
                    Change::Upsert { value } => print_entity(host, &value),
                    Change::Remove { key } => println!("[{host}] removed {key:?}"),
                }
            }
        }
        // This example opens no timelines.
        LinkEvent::Timeline { agent, .. } => println!("[{host}] timeline {agent}"),
        LinkEvent::Workflow { run, .. } => println!("[{host}] workflow {}", run.id()),
        LinkEvent::RouteWorked { route, .. } => println!("[{host}] reached over {route:?}"),
        LinkEvent::PeerRequest { request, .. } => println!("[{host}] peer request {request:?}"),
    }
}

/// Prints agents and accounts in one line each. Emails are left out.
fn print_entity(host: &str, entity: &Entity) {
    match entity {
        Entity::Agent(agent) => println!(
            "[{host}]   agent {:?} {:?} {:?} parent={} context={:?} {:?}",
            agent.vendor,
            agent.title,
            agent.activity,
            agent.parent.is_some(),
            agent.context.as_ref().map(|c| c.used_tokens),
            agent.link,
        ),
        Entity::Account(account) => {
            let windows: Vec<String> = account
                .usage
                .iter()
                .flat_map(|usage| &usage.windows)
                .map(|w| {
                    let scope = w.scope.as_deref().unwrap_or("all");
                    format!("{scope}/{}m {:.0}%", w.window_minutes, w.used_percent)
                })
                .collect();
            println!(
                "[{host}]   account {:?} plan={:?} source={:?} {windows:?} failure={:?}",
                account.vendor,
                account.plan,
                account.usage.as_ref().map(|u| u.source),
                account.usage_failure.as_ref().map(|f| &f.reason),
            );
        }
        _ => {}
    }
}

fn kind(entity: &Entity) -> &'static str {
    match entity {
        Entity::Device(_) => "device",
        Entity::DeviceSettings(_) => "device settings",
        Entity::DeviceTools(_) => "device tools",
        Entity::DeviceTasks(_) => "device tasks",
        Entity::Project(_) => "project",
        Entity::Workspace(_) => "workspace",
        Entity::Agent(_) => "agent",
        Entity::Account(_) => "account",
        Entity::Attention(_) => "attention",
        Entity::DeviceLink(_) => "link",
        Entity::Network(_) => "network",
        Entity::ListeningPort(_) => "port",
        Entity::Forward(_) => "forward",
        Entity::ProjectSettings(_) => "project settings",
        Entity::HiddenProjects(_) => "hidden projects",
        Entity::Credential(_) => "credential",
        Entity::AcpAgent(_) => "acp agent",
        Entity::TestHost(_) => "test host",
        Entity::TestLease(_) => "test lease",
        Entity::WorkflowRun(_) => "workflow run",
        Entity::SkillLibrary(_) => "skill library",
        Entity::ProjectSkills(_) => "project skills",
        Entity::SkillProposal(_) => "skill proposal",
        Entity::Phone(_) => "phone",
        Entity::PhoneBook(_) => "phone book",
        Entity::NetworkAccess(_) => "network access",
        Entity::PhoneSession(_) => "phone session",
        Entity::HostReport(_) => "host report",
        Entity::Move(_) => "move",
        Entity::AcpRegistry(_) => "acp registry",
        Entity::Dispatcher(_) => "dispatcher",
        Entity::CommandRules(_) => "command rules",
        Entity::ProjectNotes(_) => "project notes",
        Entity::AppKey(_) => "app key",
        Entity::Admission(_) => "admission",
        Entity::Terminal(_) => "terminal",
        Entity::ModelServer(_) => "model server",
        Entity::LocalModels(_) => "local models",
    }
}
