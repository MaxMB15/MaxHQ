// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: ranking and
// resolving a delegated task's mode, translated to Rust over MaxHQ's one
// permission mode, with plan as the narrowest; and the access checks a
// call passes before it runs. The original's runtime and interaction modes
// are one scale here, and its link and token checks become a permission's
// devices, vendors, accounts, places, depth and limits. A caller wakes when
// a task it started reaches an end, or asks something.

//! Whether a caller may use the agent tools for one call. Local agents,
//! access keys and calls from other devices all go through [`check`].

use std::fmt;
use std::path::{Component, Path};

use maxhq_protocol::agents::{Actor, LeaveContainer, Limits, Permission};
use maxhq_protocol::ids::{AccountId, DeviceId, PermissionId, TaskId, WorkspaceId};
use maxhq_protocol::leases::TestKind;
use maxhq_protocol::message::AgentPlace;
use maxhq_protocol::model::{Activity, PermissionMode, Vendor};
use maxhq_protocol::tasks::{Task, TaskTarget};
use maxhq_protocol::time::Timestamp;

/// What the caller reads when it has no permission at all.
pub const NOT_ALLOWED: &str =
    "You aren't allowed to run other agents. Ask the user to allow it on this agent.";

/// What the caller reads when it may lease no test machines at all.
pub const NO_LEASES: &str =
    "You aren't allowed to lease test machines. Ask the user to allow it on this agent.";

/// How much a mode allows, narrowest first.
pub fn mode_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Plan => 0,
        PermissionMode::Ask => 1,
        PermissionMode::AcceptEdits => 2,
        PermissionMode::Auto => 3,
        PermissionMode::FullAccess => 4,
    }
}

/// Whether a vendor's CLI runs in a mode. Codex has no plan mode.
pub fn offers(vendor: Vendor, mode: PermissionMode) -> bool {
    !(vendor == Vendor::Codex && mode == PermissionMode::Plan)
}

/// The mode a new agent runs at: the one asked for, never broader than its
/// caller's. With none asked, it takes the caller's, and steps down to the
/// broadest the vendor offers below it. One asked for that the vendor
/// lacks is refused. A caller with no mode of its own, such as a key, caps
/// nothing, and none asked means the device's default.
pub fn child_mode(
    caller: Option<PermissionMode>,
    asked: Option<PermissionMode>,
    vendor: Vendor,
) -> Result<Option<PermissionMode>, Refusal> {
    let Some(resolved) = asked.or(caller) else {
        return Ok(None);
    };
    if let Some(caller) = caller
        && mode_rank(resolved) > mode_rank(caller)
    {
        return Err(Refusal::BroaderMode {
            asked: resolved,
            caller,
        });
    }
    if offers(vendor, resolved) {
        return Ok(Some(resolved));
    }
    if asked.is_some() {
        return Err(Refusal::NoSuchMode {
            vendor,
            mode: resolved,
        });
    }
    ALL_MODES
        .into_iter()
        .filter(|mode| mode_rank(*mode) <= mode_rank(resolved) && offers(vendor, *mode))
        .max_by_key(|mode| mode_rank(*mode))
        .map(Some)
        .ok_or(Refusal::NoSuchMode {
            vendor,
            mode: resolved,
        })
}

/// Every mode, narrowest first.
const ALL_MODES: [PermissionMode; 5] = [
    PermissionMode::Plan,
    PermissionMode::Ask,
    PermissionMode::AcceptEdits,
    PermissionMode::Auto,
    PermissionMode::FullAccess,
];

/// What a call asks to do, as far as the check needs to know.
#[derive(Clone, Debug)]
pub enum Ask<'a> {
    /// Read, list, message, wait on or stop agents on a device.
    Act {
        /// The device.
        device: DeviceId,
    },
    /// Start an agent.
    Start(Start<'a>),
}

/// A start, as the check sees it.
#[derive(Clone, Debug)]
pub struct Start<'a> {
    /// The device it runs on.
    pub device: DeviceId,
    /// Its vendor.
    pub vendor: Vendor,
    /// Its account.
    pub account: AccountId,
    /// Where it runs, as asked.
    pub place: &'a AgentPlace,
    /// The workspace a folder belongs to, once it's looked up.
    pub workspace: Option<WorkspaceId>,
    /// The mode asked for, if any.
    pub mode: Option<PermissionMode>,
    /// The counts to hold the limits to, on the device that keeps them.
    /// None leaves the limits to that device.
    pub counts: Option<Counts>,
}

/// What a permission has used so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    /// Agents started under it, with those starting now.
    pub started: u32,
    /// Agents started under it that run now, with those starting.
    pub running: u32,
    /// The account's use in its fullest window, in percent, if known.
    pub used_percent: Option<f64>,
}

/// Who calls, under a permission.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    /// How many levels below the permission's holder it is, 0 for the
    /// holder.
    pub level: u8,
    /// Its own permission mode, or none for a key.
    pub mode: Option<PermissionMode>,
    /// The device and container workspace it runs in, or none outside a
    /// container.
    pub container: Option<(DeviceId, WorkspaceId)>,
}

/// Why a call is refused. Each reads as a sentence the caller can act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// It has no permission.
    NotAllowed,
    /// The permission has ended.
    Expired,
    /// The device isn't one it names.
    Device,
    /// The vendor isn't one it names.
    Vendor(Vendor),
    /// The account isn't one it names.
    Account,
    /// The workspace or folder isn't one it names.
    Place,
    /// The caller is as deep as the permission reaches.
    Depth,
    /// It started as many agents as it may.
    Started(u32),
    /// As many of its agents run as may.
    Running(u32),
    /// The account is above the usage cap.
    Usage(u8),
    /// The mode asked for is broader than the caller's.
    BroaderMode {
        /// The mode asked for.
        asked: PermissionMode,
        /// The caller's.
        caller: PermissionMode,
    },
    /// The vendor has no such mode.
    NoSuchMode {
        /// The vendor.
        vendor: Vendor,
        /// The mode.
        mode: PermissionMode,
    },
    /// It may lease no test machines.
    NoLeases,
    /// It may lease test machines, but not of this kind.
    TestKind(TestKind),
    /// It runs in a container, the call reaches outside it, and its
    /// permission says never.
    LeavesContainer,
    /// It runs in a container and the call reaches outside it, which its
    /// permission leaves to the user to allow call by call.
    NeedsConsent,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NotAllowed => f.write_str(NOT_ALLOWED),
            Refusal::Expired => f.write_str(
                "Your permission from the user has ended. Ask the user to allow it again.",
            ),
            Refusal::Device => f.write_str("You aren't allowed to act on that device."),
            Refusal::Vendor(vendor) => {
                write!(f, "You aren't allowed to start {} agents.", vendor.name())
            }
            Refusal::Account => f.write_str("You aren't allowed to start agents on that account."),
            Refusal::Place => {
                f.write_str("You aren't allowed to start agents in that workspace or folder.")
            }
            Refusal::Depth => f.write_str(
                "Agents at your level can't start agents of their own. Do the work yourself, \
                 or ask the user.",
            ),
            Refusal::Started(most) => write!(
                f,
                "You've started {most} agents, the most you're allowed. Ask the user to raise \
                 the limit."
            ),
            Refusal::Running(most) => write!(
                f,
                "{most} of your agents are running, the most you're allowed at once. Wait for \
                 one to finish, or stop one."
            ),
            Refusal::Usage(cap) => write!(
                f,
                "That account has used more than {cap} percent of its limit. Pick another \
                 account, or wait for it to reset."
            ),
            Refusal::BroaderMode { asked, caller } => write!(
                f,
                "A new agent can't run in {} mode, since that allows more than your own {} \
                 mode.",
                mode_name(*asked),
                mode_name(*caller)
            ),
            Refusal::NoSuchMode { vendor, mode } => {
                write!(f, "{} has no {} mode.", vendor.name(), mode_name(*mode))
            }
            Refusal::NoLeases => f.write_str(NO_LEASES),
            Refusal::TestKind(kind) => write!(
                f,
                "You aren't allowed to lease {} machines. Ask the user to allow that kind on \
                 this agent.",
                kind.name()
            ),
            Refusal::LeavesContainer => f.write_str(
                "You run in a container, and your permission doesn't let you reach outside it. \
                 You can start and reach agents only in that container. Ask the user to allow \
                 more.",
            ),
            Refusal::NeedsConsent => f.write_str(
                "You run in a container, and this call reaches outside it, so it waits for the \
                 user to allow it.",
            ),
        }
    }
}

/// Checks that a caller may reach an agent on `device`, in the container
/// workspace `target` or else on the host. A caller in a container stays in
/// that one unless its permission lets it leave. On
/// [`LeaveContainer::Ask`] the call waits for the user, which
/// [`Refusal::NeedsConsent`] tells the caller's device to ask.
pub fn check_reach(
    permission: &Permission,
    caller: Caller,
    device: DeviceId,
    target: Option<WorkspaceId>,
) -> Result<(), Refusal> {
    let Some((home, own)) = caller.container else {
        return Ok(());
    };
    if device == home && target == Some(own) {
        return Ok(());
    }
    match permission.leave_container {
        LeaveContainer::Allow => Ok(()),
        LeaveContainer::Never => Err(Refusal::LeavesContainer),
        LeaveContainer::Ask => Err(Refusal::NeedsConsent),
    }
}

/// A mode in words.
pub fn mode_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Plan => "plan",
        PermissionMode::Ask => "ask",
        PermissionMode::AcceptEdits => "accept edits",
        PermissionMode::Auto => "auto",
        PermissionMode::FullAccess => "full access",
    }
}

/// Checks one call under a permission. A start gives the mode the new
/// agent runs at, or none for the device's default.
pub fn check(
    permission: Option<&Permission>,
    caller: Caller,
    ask: &Ask,
    now: Timestamp,
) -> Result<Option<PermissionMode>, Refusal> {
    let Some(permission) = permission else {
        return Err(Refusal::NotAllowed);
    };
    let limits = permission.limits;
    if limits.expires_at.is_some_and(|at| at <= now) {
        return Err(Refusal::Expired);
    }
    let device = match ask {
        Ask::Act { device } => *device,
        Ask::Start(start) => start.device,
    };
    if !permission.devices.contains(&device) {
        return Err(Refusal::Device);
    }
    let Ask::Start(start) = ask else {
        return Ok(None);
    };
    if !permission.vendors.contains(&start.vendor) {
        return Err(Refusal::Vendor(start.vendor));
    }
    if let Some(accounts) = &permission.accounts
        && !accounts.contains(&start.account)
    {
        return Err(Refusal::Account);
    }
    if let Some(places) = &permission.places
        && !places
            .iter()
            .any(|allowed| place_allows(allowed, start.place, start.workspace))
    {
        return Err(Refusal::Place);
    }
    if caller.level.saturating_add(1) > permission.depth {
        return Err(Refusal::Depth);
    }
    if let Some(counts) = start.counts {
        if let Some(most) = limits.max_started
            && counts.started >= most
        {
            return Err(Refusal::Started(most));
        }
        if let Some(most) = limits.max_running
            && counts.running >= most
        {
            return Err(Refusal::Running(most));
        }
        if let (Some(cap), Some(used)) = (limits.usage_cap_percent, counts.used_percent)
            && used > f64::from(cap)
        {
            return Err(Refusal::Usage(cap));
        }
    }
    let mode = child_mode(caller.mode, start.mode, start.vendor)?;
    // A folder is on the host, so only the caller's own workspace stays
    // inside. This comes last, so a call that waits for the user passes
    // every other check first.
    let target = match start.place {
        AgentPlace::Workspace { workspace } => Some(*workspace),
        AgentPlace::Folder { .. } => None,
    };
    check_reach(permission, caller, start.device, target)?;
    Ok(mode)
}

/// Checks that a caller may move an agent off `from`, to start again as
/// `start` says. The user approves each move, so a caller in a container
/// may ask for one unless its permission never lets it reach outside.
pub fn check_move(
    permission: &Permission,
    caller: Caller,
    from: DeviceId,
    start: Start,
    now: Timestamp,
) -> Result<(), Refusal> {
    check(Some(permission), caller, &Ask::Act { device: from }, now)?;
    if caller.container.is_some() && permission.leave_container == LeaveContainer::Never {
        return Err(Refusal::LeavesContainer);
    }
    let caller = Caller {
        container: None,
        ..caller
    };
    let start = Start {
        mode: None,
        counts: None,
        ..start
    };
    check(Some(permission), caller, &Ask::Start(start), now).map(|_| ())
}

/// Checks one lease tool call under a permission, on the device that runs
/// it. `kind` is the kind the call leases or acts on, or none for a call
/// that names no kind, such as a list, which any kind allows. Leasing is
/// its own permission, apart from running agents.
pub fn check_lease(
    permission: Option<&Permission>,
    device: DeviceId,
    kind: Option<TestKind>,
    now: Timestamp,
) -> Result<(), Refusal> {
    let Some(permission) = permission.filter(|permission| !permission.test_kinds.is_empty()) else {
        return Err(Refusal::NoLeases);
    };
    if permission.limits.expires_at.is_some_and(|at| at <= now) {
        return Err(Refusal::Expired);
    }
    if !permission.devices.contains(&device) {
        return Err(Refusal::Device);
    }
    match kind {
        Some(kind) if !permission.test_kinds.contains(&kind) => Err(Refusal::TestKind(kind)),
        _ => Ok(()),
    }
}

/// Whether an allowed place takes the one a start asks for. A folder takes
/// every folder inside it, and a workspace takes its folders once they're
/// looked up. A folder path that climbs with `..` is never taken, since its
/// words can start inside an allowed folder and lead out of it.
fn place_allows(allowed: &AgentPlace, asked: &AgentPlace, located: Option<WorkspaceId>) -> bool {
    if let AgentPlace::Folder { path } = asked
        && Path::new(path)
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return false;
    }
    match (allowed, asked) {
        (AgentPlace::Workspace { workspace }, AgentPlace::Workspace { workspace: asked }) => {
            workspace == asked
        }
        (AgentPlace::Workspace { workspace }, AgentPlace::Folder { .. }) => {
            located == Some(*workspace)
        }
        (AgentPlace::Folder { path }, AgentPlace::Folder { path: asked }) => {
            Path::new(asked).starts_with(Path::new(path))
        }
        (AgentPlace::Folder { .. }, AgentPlace::Workspace { .. }) => false,
    }
}

/// How an agent's turn stands, for waiting on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// It's working.
    Working,
    /// It finished its turn.
    Done,
    /// It asks you something.
    NeedsInput,
    /// It stopped, or can't go on now.
    Stopped,
}

impl Turn {
    /// How a wait reports it.
    pub fn name(self) -> &'static str {
        match self {
            Turn::Working => "working",
            Turn::Done => "done",
            Turn::NeedsInput => "needs_input",
            Turn::Stopped => "stopped",
        }
    }

    /// Whether a wait ends on it.
    pub fn is_settled(self) -> bool {
        self != Turn::Working
    }
}

/// How an agent's turn stands, from its activity.
pub fn turn(activity: &Activity) -> Turn {
    match activity {
        Activity::Starting | Activity::Busy | Activity::BusyInSubagents => Turn::Working,
        Activity::DoneUnread | Activity::IdleCacheWarm { .. } | Activity::Idle => Turn::Done,
        Activity::NeedsInput { .. } => Turn::NeedsInput,
        Activity::WaitingForLimitReset { .. }
        | Activity::Paused { .. }
        | Activity::Error { .. }
        | Activity::NotAccessible => Turn::Stopped,
    }
}

/// Whether an agent that moved from `from` to `to` wakes the agent that
/// started it: a turn that ran ended, or the agent asks something or
/// stopped. Coming up idle after a start or a resume isn't a turn, so it
/// doesn't wake.
pub fn wakes(from: &Activity, to: &Activity) -> bool {
    match (turn(from), turn(to)) {
        (Turn::Working, Turn::Done) => !matches!(from, Activity::Starting),
        (Turn::Working, Turn::NeedsInput | Turn::Stopped) => true,
        _ => false,
    }
}

/// The device a task's target is on.
pub fn target_device(target: &TaskTarget) -> DeviceId {
    match target {
        TaskTarget::Agent { device, .. } | TaskTarget::New { device, .. } => *device,
    }
}

/// The ID of the permission a task the user made runs under.
pub fn user_permission(task: TaskId) -> PermissionId {
    PermissionId::derive(&format!("task:{task}"))
}

/// The permission a task the user made runs under, kept on its `home`:
/// it may act only on its target, and start only the agent it names.
pub fn task_permission(task: &Task, home: DeviceId) -> Permission {
    let (device, vendors, accounts, places, depth) = match &task.target {
        TaskTarget::Agent { device, .. } => (*device, Vec::new(), Vec::new(), Vec::new(), 0),
        TaskTarget::New { device, spec } => (
            *device,
            vec![spec.vendor],
            vec![spec.account],
            vec![spec.place.clone()],
            1,
        ),
    };
    Permission {
        id: task.permission,
        holder: Actor::Task { task: task.id },
        home,
        devices: vec![device],
        vendors,
        accounts: Some(accounts),
        places: Some(places),
        depth,
        limits: Limits::default(),
        test_kinds: Vec::new(),
        leave_container: LeaveContainer::Ask,
    }
}

/// The ID every read right carries. It's never stored, so nothing is kept
/// on it.
pub fn read_right_id() -> PermissionId {
    PermissionId::derive("read-right")
}

/// What every agent may do without a permission while its device lets
/// agents read other agents: act on `home` and `device` with the tools
/// that only look, start nothing, and lease nothing. A caller in a
/// container still asks before it reads outside it, unless `leave` says
/// otherwise.
pub fn read_right(
    holder: Actor,
    home: DeviceId,
    device: DeviceId,
    leave: LeaveContainer,
) -> Permission {
    let mut devices = vec![home];
    if device != home {
        devices.push(device);
    }
    Permission {
        id: read_right_id(),
        holder,
        home,
        devices,
        vendors: Vec::new(),
        accounts: Some(Vec::new()),
        places: Some(Vec::new()),
        depth: 0,
        limits: Limits::default(),
        test_kinds: Vec::new(),
        leave_container: leave,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::agents::DEPTH;
    use maxhq_protocol::ids::{AgentId, PermissionId};

    const NOW: Timestamp = Timestamp(1_000_000);

    fn here() -> DeviceId {
        DeviceId::derive("here")
    }

    fn account() -> AccountId {
        AccountId::derive("account")
    }

    fn permission() -> Permission {
        Permission {
            id: PermissionId::derive("p"),
            holder: Actor::Agent {
                agent: AgentId::derive("lead"),
            },
            home: here(),
            devices: vec![here()],
            vendors: vec![Vendor::Claude, Vendor::Codex],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: LeaveContainer::Ask,
        }
    }

    fn folder(path: &str) -> AgentPlace {
        AgentPlace::Folder { path: path.into() }
    }

    fn start(place: &AgentPlace) -> Start<'_> {
        Start {
            device: here(),
            vendor: Vendor::Codex,
            account: account(),
            place,
            workspace: None,
            mode: None,
            counts: Some(Counts::default()),
        }
    }

    fn holder() -> Caller {
        Caller {
            level: 0,
            mode: Some(PermissionMode::FullAccess),
            container: None,
        }
    }

    /// The holder, running in the container `boxed` on this device.
    fn boxed() -> Caller {
        Caller {
            container: Some((here(), WorkspaceId::derive("boxed"))),
            ..holder()
        }
    }

    fn checked(permission: &Permission, caller: Caller, start: Start) -> Result<(), Refusal> {
        check(Some(permission), caller, &Ask::Start(start), NOW).map(|_| ())
    }

    #[test]
    fn a_read_right_acts_on_its_devices_but_starts_and_leases_nothing() {
        let there = DeviceId::derive("there");
        let holder_actor = Actor::Agent {
            agent: AgentId::derive("reader"),
        };
        let right = read_right(holder_actor, here(), there, LeaveContainer::Ask);
        assert_eq!(right.devices, [here(), there]);
        assert_eq!(
            read_right(holder_actor, here(), here(), LeaveContainer::Ask).devices,
            [here()]
        );
        for device in [here(), there] {
            assert_eq!(
                check(Some(&right), holder(), &Ask::Act { device }, NOW),
                Ok(None)
            );
        }
        assert_eq!(
            check(
                Some(&right),
                holder(),
                &Ask::Act {
                    device: DeviceId::derive("elsewhere")
                },
                NOW
            ),
            Err(Refusal::Device)
        );
        let place = folder("/tmp/example");
        assert_eq!(
            checked(&right, holder(), start(&place)),
            Err(Refusal::Vendor(Vendor::Codex))
        );
        assert_eq!(
            check_lease(Some(&right), here(), None, NOW),
            Err(Refusal::NoLeases)
        );
        // A caller in a container asks before it reads outside it.
        assert_eq!(
            check_reach(&right, boxed(), there, None),
            Err(Refusal::NeedsConsent)
        );
        let never = read_right(holder_actor, here(), there, LeaveContainer::Never);
        assert_eq!(
            check_reach(&never, boxed(), there, None),
            Err(Refusal::LeavesContainer)
        );
    }

    #[test]
    fn a_move_needs_both_devices_and_the_vendor_there_and_asks_no_container_consent() {
        let devbox = DeviceId::derive("devbox");
        let place = folder("/work/app");
        let there = Start {
            device: devbox,
            ..start(&place)
        };
        let mut both = permission();
        both.devices.push(devbox);
        let moved = |permission: &Permission, caller: Caller, from: DeviceId| {
            check_move(permission, caller, from, there.clone(), NOW)
        };
        assert_eq!(moved(&both, holder(), here()), Ok(()));
        assert_eq!(moved(&permission(), holder(), here()), Err(Refusal::Device));
        assert_eq!(
            moved(&both, holder(), DeviceId::derive("other")),
            Err(Refusal::Device)
        );
        let mut claude = both.clone();
        claude.vendors = vec![Vendor::Claude];
        assert_eq!(
            moved(&claude, holder(), here()),
            Err(Refusal::Vendor(Vendor::Codex))
        );
        // The user approves the move, so a container's Ask needs no more.
        assert_eq!(moved(&both, boxed(), here()), Ok(()));
        let mut never = both;
        never.leave_container = LeaveContainer::Never;
        assert_eq!(
            moved(&never, boxed(), here()),
            Err(Refusal::LeavesContainer)
        );
    }

    #[test]
    fn a_folder_that_climbs_out_is_refused_whatever_it_starts_with() {
        let mut narrow = permission();
        narrow.places = Some(vec![folder("/work/allowed")]);
        for asked in [
            "/work/allowed/../other",
            "/work/allowed/../../home/me/.ssh",
            "/work/allowed/sub/../../other",
        ] {
            let place = folder(asked);
            assert_eq!(
                checked(&narrow, holder(), start(&place)),
                Err(Refusal::Place),
                "{asked}"
            );
        }
        for asked in ["/work/allowed", "/work/allowed/sub", "/work/allowed/./sub"] {
            let place = folder(asked);
            assert_eq!(checked(&narrow, holder(), start(&place)), Ok(()), "{asked}");
        }

        // A workspace's lookup can't vouch for a path that climbs either.
        let workspace = WorkspaceId::derive("allowed");
        narrow.places = Some(vec![AgentPlace::Workspace { workspace }]);
        let climbing = folder("/work/allowed/../other");
        let mut located = start(&climbing);
        located.workspace = Some(workspace);
        assert_eq!(checked(&narrow, holder(), located), Err(Refusal::Place));
        let plain = folder("/work/allowed/sub");
        let mut located = start(&plain);
        located.workspace = Some(workspace);
        assert_eq!(checked(&narrow, holder(), located), Ok(()));
    }

    #[test]
    fn each_rule_refuses_with_its_own_reason() {
        let place = folder("/work/app");
        let open = permission();
        assert_eq!(checked(&open, holder(), start(&place)), Ok(()));
        assert_eq!(
            check(None, holder(), &Ask::Act { device: here() }, NOW),
            Err(Refusal::NotAllowed)
        );

        let mut cases: Vec<(Permission, Start, Caller, Refusal)> = Vec::new();
        let mut elsewhere = start(&place);
        elsewhere.device = DeviceId::derive("elsewhere");
        cases.push((open.clone(), elsewhere, holder(), Refusal::Device));

        let mut claude_only = open.clone();
        claude_only.vendors = vec![Vendor::Claude];
        cases.push((
            claude_only,
            start(&place),
            holder(),
            Refusal::Vendor(Vendor::Codex),
        ));

        let mut one_account = open.clone();
        one_account.accounts = Some(vec![AccountId::derive("other")]);
        cases.push((one_account, start(&place), holder(), Refusal::Account));

        let mut one_place = open.clone();
        one_place.places = Some(vec![folder("/work/other")]);
        cases.push((one_place, start(&place), holder(), Refusal::Place));

        let mut shallow = open.clone();
        shallow.depth = 1;
        let child = Caller {
            level: 1,
            mode: Some(PermissionMode::FullAccess),
            container: None,
        };
        cases.push((shallow, start(&place), child, Refusal::Depth));

        let mut started = open.clone();
        started.limits.max_started = Some(2);
        let mut two = start(&place);
        two.counts = Some(Counts {
            started: 2,
            ..Counts::default()
        });
        cases.push((started, two, holder(), Refusal::Started(2)));

        let mut running = open.clone();
        running.limits.max_running = Some(1);
        let mut one = start(&place);
        one.counts = Some(Counts {
            running: 1,
            ..Counts::default()
        });
        cases.push((running, one, holder(), Refusal::Running(1)));

        let mut capped = open.clone();
        capped.limits.usage_cap_percent = Some(80);
        let mut busy = start(&place);
        busy.counts = Some(Counts {
            used_percent: Some(91.0),
            ..Counts::default()
        });
        cases.push((capped, busy, holder(), Refusal::Usage(80)));

        let mut ended = open.clone();
        ended.limits.expires_at = Some(NOW);
        cases.push((ended, start(&place), holder(), Refusal::Expired));

        let mut broader = start(&place);
        broader.mode = Some(PermissionMode::FullAccess);
        let asking = Caller {
            level: 0,
            mode: Some(PermissionMode::Ask),
            container: None,
        };
        cases.push((
            open.clone(),
            broader,
            asking,
            Refusal::BroaderMode {
                asked: PermissionMode::FullAccess,
                caller: PermissionMode::Ask,
            },
        ));

        for (permission, start, caller, refusal) in cases {
            let reason = refusal.to_string();
            assert_eq!(checked(&permission, caller, start), Err(refusal));
            assert!(reason.ends_with('.'), "{reason}");
            assert!(!reason.contains('\u{2014}') && !reason.contains('\u{2013}'));
        }
    }

    #[test]
    fn limits_that_are_off_never_refuse() {
        let place = folder("/work/app");
        let mut many = start(&place);
        many.counts = Some(Counts {
            started: 10_000,
            running: 10_000,
            used_percent: Some(100.0),
        });
        assert_eq!(checked(&permission(), holder(), many), Ok(()));
    }

    #[test]
    fn the_device_that_does_not_keep_the_counts_skips_the_limits() {
        let place = folder("/work/app");
        let mut limited = permission();
        limited.limits.max_started = Some(0);
        let mut remote = start(&place);
        remote.counts = None;
        assert_eq!(checked(&limited, holder(), remote), Ok(()));
    }

    #[test]
    fn a_folder_allows_the_folders_inside_it() {
        let mut inside = permission();
        inside.places = Some(vec![folder("/work")]);
        assert_eq!(
            checked(&inside, holder(), start(&folder("/work/app"))),
            Ok(())
        );
        assert_eq!(
            checked(&inside, holder(), start(&folder("/workshop"))),
            Err(Refusal::Place)
        );
        let workspace = WorkspaceId::derive("w");
        let mut by_workspace = permission();
        by_workspace.places = Some(vec![AgentPlace::Workspace { workspace }]);
        let place = folder("/anywhere");
        let mut located = start(&place);
        located.workspace = Some(workspace);
        assert_eq!(checked(&by_workspace, holder(), located), Ok(()));
    }

    #[test]
    fn depth_counts_levels_below_the_holder() {
        let place = folder("/work/app");
        let mut one = permission();
        one.depth = 1;
        assert_eq!(checked(&one, holder(), start(&place)), Ok(()));
        let child = Caller {
            level: 1,
            mode: None,
            container: None,
        };
        assert_eq!(checked(&permission(), child, start(&place)), Ok(()));
        let deep = Caller {
            level: DEPTH,
            mode: None,
            container: None,
        };
        assert_eq!(
            checked(&permission(), deep, start(&place)),
            Err(Refusal::Depth)
        );
    }

    #[test]
    fn a_child_mode_is_never_broader_than_its_callers() {
        use PermissionMode::*;
        assert_eq!(child_mode(Some(Auto), None, Vendor::Claude), Ok(Some(Auto)));
        assert_eq!(
            child_mode(Some(Auto), Some(Ask), Vendor::Claude),
            Ok(Some(Ask))
        );
        assert!(child_mode(Some(Ask), Some(Auto), Vendor::Claude).is_err());
        assert!(child_mode(Some(Plan), Some(Ask), Vendor::Claude).is_err());
        assert_eq!(child_mode(None, None, Vendor::Codex), Ok(None));
        assert_eq!(
            child_mode(None, Some(FullAccess), Vendor::Codex),
            Ok(Some(FullAccess))
        );
    }

    #[test]
    fn a_mode_the_vendor_lacks_steps_down_when_inherited_and_is_refused_when_asked() {
        use PermissionMode::*;
        assert_eq!(
            child_mode(Some(Plan), Some(Plan), Vendor::Codex),
            Err(Refusal::NoSuchMode {
                vendor: Vendor::Codex,
                mode: Plan
            })
        );
        // Nothing narrower than plan exists, so an inherited plan can't
        // step down either.
        assert!(child_mode(Some(Plan), None, Vendor::Codex).is_err());
        assert_eq!(child_mode(Some(Plan), None, Vendor::Claude), Ok(Some(Plan)));
    }

    #[test]
    fn turns_settle_on_done_needs_input_and_stopped() {
        assert_eq!(turn(&Activity::Busy), Turn::Working);
        assert_eq!(turn(&Activity::Idle), Turn::Done);
        assert_eq!(turn(&Activity::Error { reason: "x".into() }), Turn::Stopped);
        assert!(!Turn::Working.is_settled());
        assert!(Turn::NeedsInput.is_settled());
    }

    #[test]
    fn a_wake_comes_when_a_turn_ends_or_an_agent_asks_or_stops() {
        let error = Activity::Error { reason: "x".into() };
        assert!(wakes(&Activity::Busy, &Activity::Idle));
        assert!(wakes(&Activity::BusyInSubagents, &Activity::DoneUnread));
        assert!(wakes(&Activity::Busy, &error));
        assert!(wakes(&Activity::Starting, &error));
        assert!(!wakes(&Activity::Starting, &Activity::Idle));
        assert!(!wakes(&Activity::Idle, &Activity::DoneUnread));
        assert!(!wakes(&Activity::Idle, &Activity::Busy));
    }

    #[test]
    fn leasing_needs_its_own_permission_for_the_kind() {
        let ios = Some(TestKind::IosSimulator);
        // Allowed to run agents is not allowed to lease.
        assert_eq!(
            check_lease(Some(&permission()), here(), None, NOW),
            Err(Refusal::NoLeases)
        );
        assert_eq!(check_lease(None, here(), ios, NOW), Err(Refusal::NoLeases));

        let mut leasing = permission();
        leasing.vendors.clear();
        leasing.test_kinds = vec![TestKind::IosSimulator];
        assert_eq!(check_lease(Some(&leasing), here(), ios, NOW), Ok(()));
        assert_eq!(check_lease(Some(&leasing), here(), None, NOW), Ok(()));
        assert_eq!(
            check_lease(Some(&leasing), here(), Some(TestKind::Linux), NOW),
            Err(Refusal::TestKind(TestKind::Linux))
        );
        assert_eq!(
            check_lease(Some(&leasing), DeviceId::derive("there"), ios, NOW),
            Err(Refusal::Device)
        );
        leasing.limits.expires_at = Some(NOW);
        assert_eq!(
            check_lease(Some(&leasing), here(), ios, NOW),
            Err(Refusal::Expired)
        );
        assert!(Refusal::TestKind(TestKind::Linux)
            .to_string()
            .contains("lease linux machines"));
    }

    #[test]
    fn an_agent_in_a_container_starts_outside_it_only_as_its_permission_says() {
        let own = AgentPlace::Workspace {
            workspace: WorkspaceId::derive("boxed"),
        };
        let other = AgentPlace::Workspace {
            workspace: WorkspaceId::derive("other"),
        };
        let host = folder("/work/app");
        let there = DeviceId::derive("there");
        let mut wide = permission();
        wide.devices.push(there);
        let away = || Start {
            device: there,
            ..start(&own)
        };

        // Its own container is inside, whatever the setting.
        for leave in [
            LeaveContainer::Ask,
            LeaveContainer::Allow,
            LeaveContainer::Never,
        ] {
            wide.leave_container = leave;
            assert_eq!(checked(&wide, boxed(), start(&own)), Ok(()), "{leave:?}");
        }
        // A folder on the host, another workspace, or its container's id on
        // another device are all outside.
        for (leave, outcome) in [
            (LeaveContainer::Ask, Err(Refusal::NeedsConsent)),
            (LeaveContainer::Allow, Ok(())),
            (LeaveContainer::Never, Err(Refusal::LeavesContainer)),
        ] {
            wide.leave_container = leave;
            for asked in [start(&host), start(&other), away()] {
                assert_eq!(checked(&wide, boxed(), asked), outcome, "{leave:?}");
            }
        }
        // An agent outside any container isn't held back.
        wide.leave_container = LeaveContainer::Never;
        assert_eq!(checked(&wide, holder(), start(&host)), Ok(()));
    }

    #[test]
    fn a_start_that_waits_for_the_user_passes_every_other_check_first() {
        let host = folder("/work/app");
        let mut narrow = permission();
        narrow.vendors = vec![Vendor::Claude];
        assert_eq!(
            checked(&narrow, boxed(), start(&host)),
            Err(Refusal::Vendor(Vendor::Codex))
        );
        let mut shallow = permission();
        shallow.depth = 0;
        assert_eq!(
            checked(&shallow, boxed(), start(&host)),
            Err(Refusal::Depth)
        );
        assert_eq!(
            checked(&permission(), boxed(), start(&host)),
            Err(Refusal::NeedsConsent)
        );
    }

    #[test]
    fn an_agent_in_a_container_reaches_only_agents_in_it_unless_allowed() {
        let own = WorkspaceId::derive("boxed");
        let there = DeviceId::derive("there");
        let mut held = permission();
        for leave in [
            LeaveContainer::Ask,
            LeaveContainer::Allow,
            LeaveContainer::Never,
        ] {
            held.leave_container = leave;
            assert_eq!(check_reach(&held, boxed(), here(), Some(own)), Ok(()));
            assert_eq!(check_reach(&held, holder(), here(), None), Ok(()));
        }
        for (leave, outcome) in [
            (LeaveContainer::Ask, Err(Refusal::NeedsConsent)),
            (LeaveContainer::Allow, Ok(())),
            (LeaveContainer::Never, Err(Refusal::LeavesContainer)),
        ] {
            held.leave_container = leave;
            assert_eq!(check_reach(&held, boxed(), here(), None), outcome);
            assert_eq!(check_reach(&held, boxed(), there, Some(own)), outcome);
            let other = Some(WorkspaceId::derive("other"));
            assert_eq!(check_reach(&held, boxed(), here(), other), outcome);
        }
        assert!(Refusal::LeavesContainer
            .to_string()
            .contains("reach outside it"));
    }
}
