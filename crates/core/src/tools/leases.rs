// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The lease tools' arguments: what each takes, and its checks. They lease
//! a test machine on the caller's own device and drive it. None takes a
//! device: an agent tests on another device by starting an agent there.

use std::path::PathBuf;

use maxhq_protocol::leases::{TestKind, LONGEST_LEASE_MINUTES};
use serde_json::{json, Map, Value};

use super::agents::{id, number, text};
use super::{fields, Call, Tool};
use crate::leases::{
    LeaseCall, DEFAULT_RUN_SECONDS, DEFAULT_WAIT_SECONDS, MAX_RUN_SECONDS, MAX_WAIT_SECONDS,
};

/// The most arguments one `lease_run` command takes, with its subcommand.
const MAX_ARGS: usize = 64;

/// A short title for each lease tool.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::ListTestHosts => "List test hosts",
        Tool::LeaseOpen => "Lease a test machine",
        Tool::LeaseRun => "Run a command on a leased machine",
        Tool::LeaseScreenshot => "Screenshot a leased machine",
        Tool::LeaseInstall => "Install an app on a leased machine",
        Tool::LeaseClose => "End a lease",
        _ => unreachable!("not a lease tool"),
    }
}

/// What the agent reads about each lease tool.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::ListTestHosts => {
            "Lists the test machines this device hosts: iOS Simulators, Android emulators, \
             Linux containers and macOS VMs, with whether each is ready, what it lacks, and \
             how many wait for it. It also lists your leases, and which linked devices host \
             the kinds this one doesn't. To test on one of those, start an agent there with \
             start_agent and have it lease the machine."
        }
        Tool::LeaseOpen => {
            "Leases a test machine of one kind on this device for you alone, waiting in line \
             while it's busy. It answers the lease id and how to drive the machine. Drive a \
             Simulator or emulator with lease_run, lease_screenshot and lease_install, not with \
             xcrun simctl, idb or adb. Close the lease with lease_close when you're done, so \
             the next caller gets the machine."
        }
        Tool::LeaseRun => {
            "Runs one command on a leased machine and answers its exit code and output. On a \
             Simulator or emulator it's one agent-device command, such as [\"snapshot\", \"-i\"] \
             or [\"press\", \"@e3\"]: MaxHQ adds the flags that pick the machine, and refuses \
             commands it doesn't allow. On a Linux container it's one program and its \
             arguments, run as the test user. Output past 64 KiB is cut."
        }
        Tool::LeaseScreenshot => {
            "Takes a PNG of a leased machine's screen, scaled to at most 1280 pixels on its \
             long edge, and shows it to you."
        }
        Tool::LeaseInstall => {
            "Installs an app on a leased machine from your working folder: a .app folder or \
             .ipa for a Simulator, an .apk for an emulator, or any file or folder for a Linux \
             container, up to 4 GiB."
        }
        Tool::LeaseClose => "Ends one of your leases and frees the machine for the next caller.",
        _ => unreachable!("not a lease tool"),
    }
}

/// The JSON schema for each lease tool's arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    let lease =
        json!({"type": "string", "minLength": 1, "description": "The lease id lease_open gave."});
    let (properties, required) = match tool {
        Tool::ListTestHosts => (json!({}), json!([])),
        Tool::LeaseOpen => (
            json!({
                "kind": {
                    "type": "string",
                    "enum": TestKind::ALL.map(TestKind::name),
                    "description": "The kind of machine.",
                },
                "minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": LONGEST_LEASE_MINUTES,
                    "description": "How long the lease lasts. The device's default when left out.",
                },
                "wait_seconds": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_WAIT_SECONDS,
                    "description": format!("How long to wait while the machine is busy, {DEFAULT_WAIT_SECONDS} when left out."),
                },
            }),
            json!(["kind"]),
        ),
        Tool::LeaseRun => (
            json!({
                "lease": lease,
                "command": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "maxItems": MAX_ARGS,
                    "description": "The command and its arguments, one per item.",
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_RUN_SECONDS,
                    "description": format!("How long it may run, {DEFAULT_RUN_SECONDS} when left out."),
                },
            }),
            json!(["lease", "command"]),
        ),
        Tool::LeaseScreenshot | Tool::LeaseClose => (json!({"lease": lease}), json!(["lease"])),
        Tool::LeaseInstall => (
            json!({
                "lease": lease,
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The app, absolute or relative to your working folder.",
                },
            }),
            json!(["lease", "path"]),
        ),
        _ => unreachable!("not a lease tool"),
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Checks a lease tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = &fields(arguments)?;
    let lease = || id(args, "lease")?.ok_or_else(|| "Give a `lease`.".to_owned());
    Ok(Call::Lease(match tool {
        Tool::ListTestHosts => LeaseCall::ListTestHosts,
        Tool::LeaseOpen => LeaseCall::Open {
            kind: kind(args)?,
            minutes: number(args, "minutes", 1, LONGEST_LEASE_MINUTES.into())?
                .map(|minutes| minutes as u16),
            wait_seconds: number(args, "wait_seconds", 0, MAX_WAIT_SECONDS.into())?
                .map_or(DEFAULT_WAIT_SECONDS, |seconds| seconds as u16),
        },
        Tool::LeaseRun => LeaseCall::Run {
            lease: lease()?,
            command: command(args)?,
            timeout_seconds: number(args, "timeout_seconds", 1, MAX_RUN_SECONDS.into())?
                .map_or(DEFAULT_RUN_SECONDS, |seconds| seconds as u32),
        },
        Tool::LeaseScreenshot => LeaseCall::Screenshot { lease: lease()? },
        Tool::LeaseInstall => LeaseCall::Install {
            lease: lease()?,
            source: PathBuf::from(text(args, "path")?.ok_or("Give a `path`.")?),
        },
        Tool::LeaseClose => LeaseCall::Close { lease: lease()? },
        _ => unreachable!("not a lease tool"),
    }))
}

/// The kind `lease_open` asks for. Spaces and dashes read as underscores.
fn kind(args: &Map<String, Value>) -> Result<TestKind, String> {
    let named = text(args, "kind")?.ok_or("Give a `kind`.")?;
    TestKind::named(&named.to_lowercase().replace([' ', '-'], "_")).ok_or_else(|| {
        let kinds = TestKind::ALL.map(TestKind::name).join(", ");
        format!("`kind` can't be {named:?}. It's one of {kinds}.")
    })
}

/// The command `lease_run` runs, as a list of words.
fn command(args: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(Value::Array(words)) = args.get("command") else {
        return Err("Give a `command`, as a list of its words.".to_owned());
    };
    if words.is_empty() || words.len() > MAX_ARGS {
        return Err(format!("`command` takes 1 to {MAX_ARGS} words."));
    }
    words
        .iter()
        .map(|word| match word {
            Value::String(word) => Ok(word.clone()),
            _ => Err("Each word of `command` must be text.".to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::LeaseId;

    fn lease() -> LeaseId {
        LeaseId::derive("lease")
    }

    #[test]
    fn an_open_names_a_kind_and_may_say_how_long() {
        assert_eq!(
            Tool::LeaseOpen.parse(&json!({"kind": "iOS Simulator"})),
            Ok(Call::Lease(LeaseCall::Open {
                kind: TestKind::IosSimulator,
                minutes: None,
                wait_seconds: DEFAULT_WAIT_SECONDS,
            }))
        );
        assert_eq!(
            Tool::LeaseOpen.parse(&json!({"kind": "linux", "minutes": 60, "wait_seconds": 0})),
            Ok(Call::Lease(LeaseCall::Open {
                kind: TestKind::Linux,
                minutes: Some(60),
                wait_seconds: 0,
            }))
        );
        let refused = |arguments: Value| Tool::LeaseOpen.parse(&arguments).expect_err("refused");
        assert!(refused(json!({})).contains("`kind`"));
        assert!(refused(json!({"kind": "windows"})).contains("android_emulator"));
        assert!(refused(json!({"kind": "linux", "minutes": 241})).contains("`minutes`"));
        assert!(refused(json!({"kind": "linux", "wait_seconds": 601})).contains("`wait_seconds`"));
    }

    #[test]
    fn a_run_takes_a_command_as_words() {
        let id = lease().to_string();
        assert_eq!(
            Tool::LeaseRun.parse(&json!({"lease": id, "command": ["press", "@e3"]})),
            Ok(Call::Lease(LeaseCall::Run {
                lease: lease(),
                command: vec!["press".into(), "@e3".into()],
                timeout_seconds: DEFAULT_RUN_SECONDS,
            }))
        );
        let refused = |arguments: Value| Tool::LeaseRun.parse(&arguments).expect_err("refused");
        assert!(refused(json!({"lease": id, "command": "snapshot -i"})).contains("`command`"));
        assert!(refused(json!({"lease": id, "command": []})).contains("`command`"));
        assert!(refused(json!({"lease": id, "command": ["press", 3]})).contains("text"));
        assert!(refused(json!({"lease": id, "command": vec!["a"; MAX_ARGS + 1]})).contains("1 to"));
        assert!(
            refused(json!({"lease": id, "command": ["a"], "timeout_seconds": 601}))
                .contains("`timeout_seconds`")
        );
        assert!(refused(json!({"lease": "nope", "command": ["a"]})).contains("valid id"));
        assert!(refused(json!({"command": ["a"]})).contains("`lease`"));
    }

    #[test]
    fn the_other_tools_name_a_lease() {
        let id = lease().to_string();
        assert_eq!(
            Tool::LeaseInstall.parse(&json!({"lease": id, "path": "build/App.app"})),
            Ok(Call::Lease(LeaseCall::Install {
                lease: lease(),
                source: PathBuf::from("build/App.app"),
            }))
        );
        assert!(Tool::LeaseInstall.parse(&json!({"lease": id})).is_err());
        assert_eq!(
            Tool::LeaseScreenshot.parse(&json!({"lease": id})),
            Ok(Call::Lease(LeaseCall::Screenshot { lease: lease() }))
        );
        assert_eq!(
            Tool::LeaseClose.parse(&json!({"lease": id})),
            Ok(Call::Lease(LeaseCall::Close { lease: lease() }))
        );
        assert_eq!(
            Tool::ListTestHosts.parse(&Value::Null),
            Ok(Call::Lease(LeaseCall::ListTestHosts))
        );
    }

    #[test]
    fn the_lease_tools_reach_keys_but_not_the_agent_permission() {
        for tool in Tool::LEASES {
            assert_eq!(Tool::named(tool.name()), Some(tool));
            assert!(Tool::ALL.contains(&tool));
            assert!(tool.for_key() && !tool.runs_agents());
        }
        for call in [
            LeaseCall::ListTestHosts,
            LeaseCall::Close { lease: lease() },
        ] {
            let call = Call::Lease(call);
            assert_eq!(call.device(), None);
            assert!(Tool::LEASES.contains(&call.tool()));
        }
    }
}
