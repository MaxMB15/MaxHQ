// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Stable IDs.
//!
//! Each kind of ID is its own type, so an agent ID can't be passed where a
//! session ID is expected. IDs for things MaxHQ creates are UUIDv7, which
//! sort by creation time. IDs for things `maxd` finds, such as a session you
//! started in your own terminal or a repository it saw checked out, are
//! UUIDv5 of what identifies them. Finding the same thing again, or on
//! another device, then gives the same ID with nothing to look up.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;
use uuid::Uuid;

/// The UUIDv5 namespace for every derived MaxHQ ID. Changing it changes
/// every derived ID.
const NAMESPACE: Uuid = Uuid::from_u128(0x3cda8af9_1bdc_4677_b1b5_c2d1b47a26be);

macro_rules! uuid_ids {
    ($($(#[$meta:meta])* $name:ident;)+) => {$(
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize, Type, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Makes a new ID from the current time and random bits.
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }

            /// The ID for whatever `name` identifies. The same name always
            /// gives the same ID, and the same name gives a different ID
            /// for each kind of ID.
            pub fn derive(name: &str) -> Self {
                let name = format!("{}:{name}", stringify!($name));
                Self(Uuid::new_v5(&NAMESPACE, name.as_bytes()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    )+};
}

uuid_ids! {
    /// A machine that runs `maxd`.
    DeviceId;
    /// A project, which is one repository however many places it's checked out.
    ProjectId;
    /// A checkout of a project on a device, either a folder or a container.
    WorkspaceId;
    /// An agent CLI session, or a sub-agent inside one.
    AgentId;
    /// A process kept alive by `maxd hold`.
    SessionId;
    /// Something an agent needs from you, such as a permission prompt.
    AttentionId;
    /// A vendor login on a device.
    AccountId;
    /// A client connection, such as the desktop app on one Mac.
    ClientId;
    /// One run of `maxd`, new each time it starts. Sequence numbers start
    /// over with it.
    InstanceId;
    /// A device you added to a client, which keeps it until you remove it.
    /// The client knows a device's route before it learns its ID.
    LinkId;
    /// An overlay network a client reaches devices through.
    NetworkId;
    /// Makes an action safe to send twice. `maxd` acts on a key once and
    /// answers every repeat with the first result.
    IdempotencyKey;
    /// A file or folder staged to send to an agent.
    AttachmentId;
    /// One upload or download of files between a client and a device.
    TransferId;
    /// A port a process in a workspace listens on.
    PortId;
    /// A client's local port that carries connections to a [`PortId`].
    ForwardId;
    /// One connection through a forward.
    StreamId;
    /// A login or key a device holds.
    CredentialId;
    /// A Sign in with ChatGPT round that a device started and waits on.
    SignInId;
    /// What one agent or access key may do with the agent tools.
    PermissionId;
    /// An access key, which lets a tool outside MaxHQ call the agent tools.
    KeyId;
    /// A scheduled task, which starts or messages an agent on a timer, a
    /// webhook or GitHub events.
    TaskId;
    /// A local ACP command defined on a device.
    AcpAgentId;
    /// A test host: one kind of test machine on one backend on one device,
    /// derived from the three.
    TestHostId;
    /// One lease of a test host.
    LeaseId;
    /// One move of a workspace the old scripts made into MaxHQ.
    MoveId;
    /// One run of a workflow, derived from its key.
    WorkflowRunId;
    /// A git repo or local folder the skill library takes skills from.
    SkillSourceId;
    /// A changed skill an agent proposes, waiting for you.
    SkillProposalId;
    /// A phone you paired, from its pairing on.
    PhoneId;
    /// One pairing that waits for its phone, until it's used or expires.
    PairId;
    /// A token one device holds for one phone, kept across refreshes.
    PhoneSessionId;
    /// A project note, picked by whichever device or client creates it.
    NoteId;
    /// An app key one device trusts, derived from the device and the key.
    AppKeyId;
    /// A card that asks whether to let in a program that proved no app key
    /// the device trusts.
    AdmissionId;
}

/// The boot a device or container is on.
///
/// Linux reads it from `/proc/sys/kernel/random/boot_id` and macOS from
/// `kern.bootsessionuuid`. A new value means the machine restarted since it
/// was last seen. Incus containers get a fresh one each time they start. The
/// value is opaque, so it's a string rather than a UUID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(transparent)]
pub struct BootId(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_as_bare_strings() {
        let id: AgentId = "0199a7c2-5f3e-7d10-8a4b-2c6e9f1d3b57".parse().unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"0199a7c2-5f3e-7d10-8a4b-2c6e9f1d3b57\"");
        assert_eq!(serde_json::from_str::<AgentId>(&json).unwrap(), id);
    }

    #[test]
    fn generated_ids_sort_by_creation() {
        let first = SessionId::generate();
        let second = SessionId::generate();
        assert!(first < second);
        assert_eq!(first.0.get_version_num(), 7);
    }

    #[test]
    fn derived_ids_depend_on_the_name_and_the_kind() {
        let id = ProjectId::derive("git:github.com/example-user/maxhq");
        assert_eq!(id, ProjectId::derive("git:github.com/example-user/maxhq"));
        assert_eq!(id.0.get_version_num(), 5);
        assert_ne!(id, ProjectId::derive("git:github.com/example-user/comet"));
        assert_ne!(
            id.0,
            WorkspaceId::derive("git:github.com/example-user/maxhq").0
        );
    }
}
