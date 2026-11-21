// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where the webview's view comes from. The app's backend is one source and
// sample data is the other, and `VITE_MAXHQ_DATA` picks between them. Until
// the backend lands, dev builds use the sample data unless told otherwise.

import type {
	AccessKey,
	AccountId,
	AcpAgent,
	AcpAgentId,
	AcpEnvValue,
	AdmissionId,
	AgentDefaults,
	ActionOutcome,
	AppKeyState,
	AgentExcerpt,
	AgentId,
	AgentPlace,
	AgentSpec,
	AttentionAnswer,
	AttentionId,
	ChatAct,
	CloneProgress,
	CloneStage,
	CloneState,
	CommandRule,
	CommandError,
	CopiedProject,
	CopyTarget,
	CredentialId,
	Delta,
	DeviceId,
	EditorChoice,
	Editors,
	FindAt,
	ForwardId,
	FunnelState,
	GitIdentity,
	HookAddress,
	IgnoredFile,
	IgnoredFiles,
	InboxChange,
	InlineRef,
	ItemId,
	LeaseId,
	Limits,
	LinkClaimDetails,
	LinkId,
	McpCommand,
	ModelAction,
	MoveId,
	MoveStep,
	MoveTo,
	NodeDetail,
	NoteId,
	NoteVersion,
	NoteVersionInfo,
	PathMatch,
	PeerLink,
	Permission,
	PermissionId,
	PermissionMode,
	PhoneDevice,
	PhoneId,
	PhonePairing,
	PhoneSessionId,
	PlaceFailure,
	PortId,
	ProcessList,
	ProcessSignal,
	ProjectId,
	ProjectSettings,
	ProposalAction,
	ProposalSeen,
	PullAction,
	PullListState,
	Run,
	RunKey,
	SavedNote,
	SendMode,
	Shown,
	SignInId,
	SkillLock,
	SkillProposalId,
	SkillSourceId,
	Snapshot,
	SourceKind,
	SourceTrust,
	SshIncludeState,
	Task,
	TakeOverMode,
	TaskDelivery,
	TaskId,
	TestHostId,
	TimelineFind,
	TimelineUpdate,
	Timestamp,
	ToolKind,
	ToolPlace,
	TransferId,
	UploadTarget,
	UserInput,
	VersionHash,
	WorkflowUpdate,
	WorkspaceId,
	WorkspaceSpec,
} from "../bindings";
import type {
	AppInfo,
	DeviceSettings,
	Diagnostics,
	FleetProgress,
	HostLoad,
	SetupHost,
	SetupProgress,
	SetupSecret,
	ToolTarget,
} from "../fleet/wire";
import { DEPTH } from "../bindings";
import type { FolderCalls } from "../folders/requests";
import type { NoticeCalls } from "../notices/requests";
import type { GitItemRef } from "../pills/draft";
import type { TerminalCalls } from "../terminal/requests";
import type { SessionCalls } from "../terminal/session";
import type { TerminalStream } from "../terminal/stream";

export type {
	AccessKey,
	AgentDefaults,
	CloneProgress,
	CloneStage,
	CloneState,
	CommandRule,
	CopiedProject,
	CopyTarget,
	EditorChoice,
	Editors,
	GitItemRef,
	IgnoredFile,
	IgnoredFiles,
	McpCommand,
	ModelAction,
	PathMatch,
	PlaceFailure,
	SshIncludeState,
};

/** Stops a subscription. */
export type Unsubscribe = () => void;

/** A node's detail, or why it didn't come. */
export type NodeDetailResult =
	{ status: "ok"; data: NodeDetail } | { status: "error"; error: CommandError };

/** What a command that can fail returns. */
export type CommandResult = { status: "ok"; data: null } | { status: "error"; error: CommandError };

/** What an action came back with. */
export type ActionResult =
	{ status: "ok"; data: ActionOutcome } | { status: "error"; error: CommandError };

/** How far one upload or download has got. */
export type TransferProgress = {
	/** The transfer this is about. */
	transfer: TransferId;
	/** Bytes sent so far, across every file in it. */
	sent: number;
	/** Bytes in all. */
	total: number;
	/** The file on its way now, as a path inside the transfer. */
	file: string;
	/** Waiting means the device has no room yet, and paused means the link dropped. */
	state: "uploading" | "waiting" | "paused" | "done" | "failed";
};

/** Where a download landed on this Mac, or why it didn't. */
export type DownloadResult =
	{ status: "ok"; data: { path: string } } | { status: "error"; error: CommandError };

/**
 * A picture or video ready for the webview at `url`, or one larger than the
 * limit asked, which stayed on its device, with its size. Or a path the
 * device refused, saying whether it's outside the agent's folders, which
 * Show anyway can still fetch.
 */
export type MediaResult =
	| {
			status: "ok";
			data:
				| { kind: "cached"; url: string; size: number }
				| { kind: "too_large"; size: number }
				| { kind: "refused"; outside: boolean; message: string };
	  }
	| { status: "error"; error: CommandError };

/**
 * The forward that opened and the port it listens on here, or why it didn't
 * open. The forward is null until the device has answered.
 */
export type ForwardResult =
	| { status: "ok"; data: { forward: ForwardId | null; local_port: number } }
	| { status: "error"; error: CommandError };

/** What the `#` menu found for an agent's repository. */
export type GitItemSearch = {
	/** The agent's repository, as "owner/name", or null when it has none on GitHub. */
	repo: string | null;
	items: GitItemRef[];
	/** Why the lookup couldn't run, such as no GitHub App for the repository. */
	unavailable: string | null;
};

/** One device's latest load sample, or null when it has none to give. */
export type DeviceLoad = {
	device: DeviceId;
	load: HostLoad | null;
	/** When this client got the sample, by its own clock. */
	received_at: Timestamp;
};

/** The processes an agent started, or why there's no list. */
export type ProcessesResult =
	{ status: "ok"; data: ProcessList } | { status: "error"; error: CommandError };

/** A note version, or why it couldn't be read. */
export type NoteResult =
	{ status: "ok"; data: NoteVersion } | { status: "error"; error: CommandError };

/** A note's new head after a save, or why the save failed. */
export type SaveNoteResult =
	{ status: "ok"; data: SavedNote } | { status: "error"; error: CommandError };

/** A note's versions, newest first, or why they couldn't be read. */
export type NoteHistoryResult =
	{ status: "ok"; data: NoteVersionInfo[] } | { status: "error"; error: CommandError };

/** What a note save sends besides which note it is. */
export type NoteSave = {
	/** The version the text was edited from, or null for a new note. */
	base: VersionHash | null;
	title: string;
	body: string;
	/** Whether the note is deleted. Restoring one saves it with this false. */
	deleted: boolean;
};

/** A device's load over a window, or why it couldn't be read. */
export type DiagnosticsResult =
	{ status: "ok"; data: Diagnostics } | { status: "error"; error: CommandError };

/** The link a set-up device got, or why setup stopped. */
export type SetupResult = { status: "ok"; data: LinkId } | { status: "error"; error: CommandError };

/** A device's permissions, or every device's together, or why they couldn't be read. */
export type PermissionsResult =
	{ status: "ok"; data: Permission[] } | { status: "error"; error: CommandError };

/** A device's access keys, or every device's together, or why they couldn't be read. */
export type KeysResult =
	{ status: "ok"; data: AccessKey[] } | { status: "error"; error: CommandError };

/** The other devices one device links to, or why they couldn't be read. */
export type DeviceLinksResult =
	{ status: "ok"; data: PeerLink[] } | { status: "error"; error: CommandError };

/** What the question about a held link shows, or why it couldn't be read. */
export type LinkClaimDetailsResult =
	{ status: "ok"; data: LinkClaimDetails } | { status: "error"; error: CommandError };

/** This Mac's app key, or why it couldn't be read. */
export type AppKeyResult =
	{ status: "ok"; data: AppKeyState } | { status: "error"; error: CommandError };

/** The devices a phone can pair with, or why they couldn't be read. */
export type PhoneDevicesResult =
	{ status: "ok"; data: PhoneDevice[] } | { status: "error"; error: CommandError };

/** A pairing that waits for its phone, or why it couldn't start. */
export type PhonePairResult =
	{ status: "ok"; data: PhonePairing } | { status: "error"; error: CommandError };

/** The names of the devices a phone's switch didn't reach, or why it couldn't go out. */
export type PhoneEverythingResult =
	{ status: "ok"; data: string[] } | { status: "error"; error: CommandError };

/** The names of the devices that made no new code for a phone, or why none went out. */
export type PhoneCodesResult =
	{ status: "ok"; data: string[] } | { status: "error"; error: CommandError };

/** A device's scheduled tasks, or every device's together, or why they couldn't be read. */
export type TasksResult = { status: "ok"; data: Task[] } | { status: "error"; error: CommandError };

/** A task's recent runs, newest first, or why they couldn't be read. */
export type RunsResult = { status: "ok"; data: Run[] } | { status: "error"; error: CommandError };

/** A task's recent webhook requests or GitHub events, newest first, or why they couldn't be read. */
export type DeliveriesResult =
	{ status: "ok"; data: TaskDelivery[] } | { status: "error"; error: CommandError };

/** Whether a device's webhooks reach the internet, or why that couldn't be read. */
export type FunnelResult =
	{ status: "ok"; data: FunnelState } | { status: "error"; error: CommandError };

/** Limits with each one off. */
export const NO_LIMITS: Limits = {
	max_started: null,
	max_running: null,
	usage_cap_percent: null,
	expires_at: null,
};

/**
 * What a new permission starts with until you change it: no limits, and as
 * many levels deep as `maxd`'s `DEPTH`.
 */
export const AGENT_DEFAULTS: AgentDefaults = { limits: NO_LIMITS, depth: DEPTH };

/** A named setup the sample data can switch to, so every connection state can be seen. */
export type Scenario = {
	/** Stable across renders. */
	readonly id: string;
	/** The menu label. */
	readonly label: string;
};

/** Extra controls only the sample data has. */
export type MockControls = {
	/** The setups it can switch to. */
	readonly scenarios: readonly Scenario[];
	/** The setup in use. */
	readonly scenario: () => string;
	/** Switches to another setup, sending the change as a delta. */
	readonly setScenario: (id: string) => void;
};

/** The calls that live with their own helpers: the terminal's, the session's and the folders'. */
type HelperCalls = TerminalCalls & SessionCalls & FolderCalls & NoticeCalls;

/**
 * One place the view comes from, with the commands that act on it. The
 * terminal and folder calls live with their helpers.
 */
export type DataSource = HelperCalls & {
	/** Which implementation this is. */
	readonly kind: "tauri" | "native" | "mock";
	/** Terminals' bytes both ways, where this source carries them. */
	readonly terminalStream?: TerminalStream;
	/** The whole view now. */
	readonly snapshot: () => Promise<Snapshot>;
	/** Calls `onDelta` with each change. Subscribe before loading the snapshot. */
	readonly subscribe: (onDelta: (delta: Delta) => void) => Promise<Unsubscribe>;
	/** Calls `onFocus` when a notification click asks to open an agent. */
	readonly onFocusAgent: (onFocus: (agent: AgentId) => void) => Promise<Unsubscribe>;
	/** The app's version and protocol, or null where there's no backend to ask. */
	readonly appInfo: () => Promise<AppInfo | null>;
	/** Marks a done agent as seen, which clears its unread ring. */
	readonly markSeen: (agent: AgentId) => Promise<CommandResult>;
	/** Checks usage now, for one account or all of them. */
	readonly refreshUsage: (account: AccountId | null) => Promise<CommandResult>;
	/** Retries one device's connection now, or every device's. */
	readonly refreshLinks: (link: LinkId | null) => Promise<CommandResult>;
	/** Tells the backend what this window's panes show, so it skips alerts you can already see. */
	readonly setShown: (shown: Shown) => Promise<void>;
	/**
	 * Calls `onUpdate` with an agent's newest items, then with each change, until unsubscribed.
	 * `live` is set for changes that just happened, and clear for what an open or a reconnect
	 * caught up on.
	 */
	readonly openTimeline: (
		agent: AgentId,
		onUpdate: (update: TimelineUpdate, live: boolean) => void
	) => Promise<Unsubscribe>;
	/** Asks for the items before ordinal `before`, which arrive as an `older` update. */
	readonly loadOlder: (agent: AgentId, before: number) => Promise<CommandResult>;
	/** Starts a new agent. */
	readonly startAgent: (spec: AgentSpec) => Promise<ActionResult>;
	/** Sends a message to an agent. */
	readonly sendInput: (agent: AgentId, input: UserInput, mode: SendMode) => Promise<ActionResult>;
	/** Takes back a queued message before it's sent. */
	readonly cancelQueued: (agent: AgentId, item: ItemId) => Promise<ActionResult>;
	/** Stops an agent's current turn. */
	readonly interrupt: (agent: AgentId) => Promise<ActionResult>;
	/** Changes an agent's permission mode. */
	readonly setPermission: (agent: AgentId, mode: PermissionMode) => Promise<ActionResult>;
	/**
	 * Changes an agent's model. With `account`, another vendor's login on
	 * the agent's device, the agent moves there with its conversation.
	 */
	readonly setModel: (
		agent: AgentId,
		model: string,
		account?: AccountId
	) => Promise<ActionResult>;
	/**
	 * Changes the effort level a Claude Code or Codex agent runs at, from
	 * its next message. Null goes back to the vendor's default.
	 */
	readonly setEffort: (agent: AgentId, effort: string | null) => Promise<ActionResult>;
	/** Ends an agent's process. Its conversation stays. */
	readonly stopAgent: (agent: AgentId) => Promise<ActionResult>;
	/**
	 * Deletes a lead agent MaxHQ holds, with its sub-agents, stopping it
	 * first. Its folder and its CLI's transcript stay.
	 */
	readonly deleteAgent: (agent: AgentId) => Promise<ActionResult>;
	/**
	 * Moves a lead agent MaxHQ holds on `from` to another device, through
	 * the `maxd` on `device`, the one the app runs with. A new agent starts
	 * there from a summary of the conversation, the old one stops and
	 * settles, and the tasks that send to it follow. `stop` stops a turn
	 * that runs or a prompt that waits first. Answers with the new agent.
	 */
	readonly moveAgent: (
		device: DeviceId,
		agent: AgentId,
		from: DeviceId,
		to: MoveTo,
		stop: boolean
	) => Promise<ActionResult>;
	/**
	 * Names an agent, held or seen outside MaxHQ, over the title its CLI
	 * gives. A blank title clears the name.
	 */
	readonly renameAgent: (agent: AgentId, title: string) => Promise<ActionResult>;
	/**
	 * Changes where agents sit in the sidebar: settles, snoozes or pins
	 * them. One result for each agent, in the same order.
	 */
	readonly setInbox: (agents: AgentId[], change: InboxChange) => Promise<ActionResult[]>;
	/** Takes over an agent you started in your own terminal. */
	readonly continueHere: (agent: AgentId) => Promise<ActionResult>;
	/**
	 * Stops the CLI of an agent you started in your own terminal, now or
	 * once its turn ends, and continues it here. `cancel` stops a wait.
	 */
	readonly takeOver: (agent: AgentId, mode: TakeOverMode) => Promise<ActionResult>;
	/**
	 * Files and folders in an agent's folder for the `@` menu, relative to
	 * it and best first, at most 50. What `.gitignore` leaves out isn't there.
	 */
	readonly findPaths: (agent: AgentId, query: string) => Promise<PathMatch[]>;
	/**
	 * Pull requests and issues in the GitHub repo of an agent's folder, for
	 * the `#` menu. A query of digits looks that number up first. `repo`,
	 * "owner/name", looks in that repo instead, such as one a pasted link
	 * names, with what the agent's device can already read.
	 */
	readonly findGitItems: (agent: AgentId, query: string, repo?: string) => Promise<GitItemSearch>;
	/** An agent's timeline as Markdown of at most `budget` characters, to attach as a file. */
	readonly excerptAgent: (agent: AgentId, budget: number) => Promise<AgentExcerpt>;
	/**
	 * Looks for `query` across an agent's whole timeline, older pages too,
	 * and makes the match `at` names active. Matches ignore case.
	 */
	readonly findInTimeline: (agent: AgentId, query: string, at: FindAt) => Promise<TimelineFind>;
	/**
	 * Answers an approval or a question. `refs` are the pills in what you
	 * typed, which go only to a maxd that takes them.
	 */
	readonly answer: (
		event: AttentionId,
		answer: AttentionAnswer,
		refs?: readonly InlineRef[]
	) => Promise<ActionResult>;
	/** Readies files or folders on a device to send, one result for each path. */
	readonly stageAttachments: (device: DeviceId, paths: string[]) => Promise<ActionResult[]>;
	/**
	 * Readies files or folders the `@` menu found in an agent's folder, by
	 * the paths `findPaths` gave, one result for each. They stage on the
	 * agent's device from the folder the search looked in.
	 */
	readonly stageFound: (agent: AgentId, paths: string[]) => Promise<ActionResult[]>;
	/** Readies pasted bytes to send to an agent on a device. */
	readonly stagePasted: (
		device: DeviceId,
		name: string,
		mime: string,
		bytes: Uint8Array
	) => Promise<ActionResult>;
	/** Changes a device's settings. */
	readonly setDeviceSettings: (settings: DeviceSettings) => Promise<ActionResult>;
	/** Makes a container workspace for a project. It shows its progress as it goes. */
	readonly createWorkspace: (spec: WorkspaceSpec) => Promise<ActionResult>;
	/** Starts a stopped container workspace. */
	readonly startWorkspace: (workspace: WorkspaceId) => Promise<ActionResult>;
	/** Stops a container workspace, keeping its files. */
	readonly stopWorkspace: (workspace: WorkspaceId) => Promise<ActionResult>;
	/** Deletes a container workspace. Unpushed work stops it unless `discardUnpushed` is set. */
	readonly deleteWorkspace: (
		workspace: WorkspaceId,
		discardUnpushed: boolean
	) => Promise<ActionResult>;
	/**
	 * The files git ignores in a workspace's checkout, sorted by path, for a
	 * copy to take along. It throws with the reason when the device can't say.
	 */
	readonly listIgnoredFiles: (workspace: WorkspaceId) => Promise<IgnoredFiles>;
	/**
	 * Copies a workspace's project to a new container or a clone in a folder.
	 * Once that's ready, the chosen `files` come over from the source and land
	 * beside what git tracks. `onProgress` hears the files move. It throws
	 * with the reason when the copy can't go ahead, and otherwise answers with
	 * the new workspace and the files that went in or didn't.
	 */
	readonly copyProject: (
		from: WorkspaceId,
		target: CopyTarget,
		files: string[],
		onProgress: (progress: TransferProgress) => void
	) => Promise<CopiedProject>;
	/** Stops a folder's clone and removes what it cloned so far. */
	readonly cancelClone: (workspace: WorkspaceId) => Promise<ActionResult>;
	/** Runs a failed or cancelled clone again. */
	readonly retryClone: (workspace: WorkspaceId) => Promise<ActionResult>;
	/** Forgets a folder whose clone failed or was cancelled. */
	readonly removeClone: (workspace: WorkspaceId) => Promise<ActionResult>;
	/**
	 * Where an editor stands with a container workspace over SSH: whether
	 * your `~/.ssh/config` includes MaxHQ's yet, or why it can't reach it.
	 */
	readonly sshInclude: (workspace: WorkspaceId) => Promise<SshIncludeState>;
	/**
	 * Adds the `Include` line for MaxHQ's SSH config at the top of your
	 * `~/.ssh/config`, or with `add` false remembers that you said no.
	 */
	readonly setupSshInclude: (workspace: WorkspaceId, add: boolean) => Promise<SshIncludeState>;
	/** The editors on this Mac a workspace can open in, in the menu's order. */
	readonly listEditors: () => Promise<Editors>;
	/**
	 * Opens a workspace in an editor, or the file at `path` in it at `line`
	 * and `column`. A relative path is in the workspace's checkout, and one on
	 * another device opens over ssh.
	 */
	readonly openInEditor: (
		editor: string,
		workspace: WorkspaceId,
		path?: string,
		line?: number,
		column?: number
	) => Promise<CommandResult>;
	/** Changes one project's settings on one device. */
	readonly setProjectSettings: (settings: ProjectSettings) => Promise<ActionResult>;
	/** Hides a project from the sidebar on every device, or shows it again. */
	readonly setProjectHidden: (project: ProjectId, hidden: boolean) => Promise<ActionResult>;
	/** Takes one "Always allow" command rule off a project on the device that keeps it. */
	readonly removeCommandRule: (
		device: DeviceId,
		project: ProjectId,
		rule: CommandRule
	) => Promise<ActionResult>;
	/** This Mac's global git name and email, the author a project's commits default to. */
	readonly gitIdentity: () => Promise<GitIdentity>;
	/** Winds a device's agents down before it restarts, waiting at most `windowSecs`. */
	readonly windDown: (device: DeviceId, windowSecs: number) => Promise<ActionResult>;
	/**
	 * Restarts the `maxd` behind a link through its service manager, on this
	 * Mac or over SSH, so it works on one too old for anything else. Mac only.
	 */
	readonly restartMaxd: (link: LinkId) => Promise<CommandResult>;
	/**
	 * Forgets a device you added and closes its link. Every other device
	 * takes out its key and cuts its link to it. This Mac stays. Mac only.
	 */
	readonly removeDevice: (link: LinkId) => Promise<CommandResult>;
	/**
	 * Installs this Mac's `maxd` as a login service and starts it, then
	 * connects to it. Mac only.
	 */
	readonly startLocalMaxd: () => Promise<CommandResult>;
	/**
	 * Stops this Mac's `maxd` and every agent on it, and takes it out of the
	 * login items. With `deleteData` it also deletes MaxHQ's data on this Mac,
	 * and the app quits. Mac only.
	 */
	readonly uninstallLocalMaxd: (deleteData: boolean) => Promise<CommandResult>;
	/**
	 * Sends a Claude setup-token to each device, one result for each. The
	 * webview drops the token once it's sent.
	 */
	readonly storeClaudeToken: (
		devices: DeviceId[],
		origin: AccountId,
		label: string,
		token: string
	) => Promise<ActionResult[]>;
	/**
	 * Sends a Cursor API key to each device, one result for each. The
	 * webview drops the key once it's sent.
	 */
	readonly storeCursorKey: (
		devices: DeviceId[],
		label: string,
		token: string
	) => Promise<ActionResult[]>;
	/** Runs a whole Sign in with ChatGPT round for a device, browser and callback included. */
	readonly signInChatGpt: (device: DeviceId) => Promise<ActionResult>;
	/**
	 * Sends a GitHub App to each device, one result for each. The backend
	 * reads the key at `keyPath`, so the webview never holds it.
	 */
	readonly setGithubApp: (
		devices: DeviceId[],
		appId: number,
		keyPath: string
	) => Promise<ActionResult[]>;
	/** Has a device check its GitHub App now, instead of at its next check. */
	readonly checkGithubApp: (device: DeviceId) => Promise<ActionResult>;
	/**
	 * Sends the APNs key that pushes to phones to each device, one result
	 * for each. The backend reads the `.p8` at `keyPath`, so the webview
	 * never holds it.
	 */
	readonly setApnsKey: (
		devices: DeviceId[],
		keyId: string,
		teamId: string,
		keyPath: string
	) => Promise<ActionResult[]>;
	/** Makes a device forget a login or key it holds. */
	readonly forgetCredential: (
		device: DeviceId,
		credential: CredentialId
	) => Promise<ActionResult>;
	/**
	 * Saves a local ACP agent on a device, or replaces the one with its ID.
	 * `env` holds only the values that are new or changed, and the webview
	 * drops them once they're sent.
	 */
	readonly setAcpAgent: (
		device: DeviceId,
		agent: AcpAgent,
		env: AcpEnvValue[]
	) => Promise<ActionResult>;
	/** Removes a local ACP agent and its values from a device. */
	readonly deleteAcpAgent: (device: DeviceId, id: AcpAgentId) => Promise<ActionResult>;
	/**
	 * Acts on a device's local models: adds, tests or removes a server URL,
	 * downloads, pauses, resumes or deletes a model file, or loads or
	 * unloads a model. Adding a server answers with the new server's ID. The
	 * default model is a device setting, saved with `setDeviceSettings`.
	 */
	readonly modelAction: (device: DeviceId, action: ModelAction) => Promise<ActionResult>;
	/** Sends files or folders on this Mac to a device, one result for each path. */
	readonly upload: (
		device: DeviceId,
		target: UploadTarget,
		paths: string[],
		onProgress: (progress: TransferProgress) => void
	) => Promise<ActionResult[]>;
	/** Sends pasted bytes to a device. */
	readonly uploadPasted: (
		device: DeviceId,
		target: UploadTarget,
		name: string,
		mime: string,
		bytes: Uint8Array,
		onProgress: (progress: TransferProgress) => void
	) => Promise<ActionResult>;
	/**
	 * Readies a file `from` can see, such as a picture in its timeline, for
	 * an agent on `device`. It comes through `from`'s device to this Mac, then
	 * uploads to `target` when one is given and stages on `device` otherwise.
	 * `onProgress` hears both legs, each under its own transfer.
	 */
	readonly attachFromAgent: (
		device: DeviceId,
		target: UploadTarget | null,
		from: AgentId,
		path: string,
		onProgress: (progress: TransferProgress) => void
	) => Promise<ActionResult>;
	/** Stops an upload or download. */
	readonly cancelTransfer: (transfer: TransferId) => Promise<void>;
	/** Fetches a file an agent can see into ~/Downloads/MaxHQ. */
	readonly download: (
		agent: AgentId,
		path: string,
		onProgress: (progress: TransferProgress) => void
	) => Promise<DownloadResult>;
	/**
	 * Fetches a picture or video an agent can see into the app's media
	 * cache, unless it's larger than `limit` bytes. `anyway` asks for one
	 * file outside the agent's folders, and only a click on Show anyway
	 * sets it.
	 */
	readonly media: (
		agent: AgentId,
		path: string,
		limit: number,
		anyway: boolean,
		onProgress: (progress: TransferProgress) => void
	) => Promise<MediaResult>;
	/** Forwards a port in a workspace to the same port here, or the next free one. */
	readonly openForward: (port: PortId) => Promise<ForwardResult>;
	/** Closes a forward this window opened. */
	readonly closeForward: (forward: ForwardId) => Promise<void>;
	/** Opens http://localhost at a port a Mac agent listens on, in the browser. */
	readonly openLocalhost: (port: number) => Promise<void>;
	/** The processes in an agent's tree now, read from its device for the Running list. */
	readonly listProcesses: (agent: AgentId) => Promise<ProcessesResult>;
	/**
	 * Sends `signal` to one process the Running list showed, named by its pid
	 * and start. Its device sends it only while that process is in the tree.
	 */
	readonly signalProcess: (
		agent: AgentId,
		pid: number,
		start: number,
		signal: ProcessSignal
	) => Promise<ActionResult>;
	/** One version of a note from a device's copy, its head when `version` is null. */
	readonly readNote: (
		device: DeviceId,
		project: ProjectId,
		note: NoteId,
		version: VersionHash | null
	) => Promise<NoteResult>;
	/**
	 * Saves a note on a device. A null base creates it. A base that's no longer
	 * the head merges with the head, and the answer says it merged.
	 */
	readonly saveNote: (
		device: DeviceId,
		project: ProjectId,
		note: NoteId,
		save: NoteSave
	) => Promise<SaveNoteResult>;
	/** A note's versions on a device, newest first, without their bodies. */
	readonly noteHistory: (
		device: DeviceId,
		project: ProjectId,
		note: NoteId
	) => Promise<NoteHistoryResult>;
	/** Each device's latest CPU and memory sample, for picking where an agent runs. */
	readonly readLoad: (devices: DeviceId[]) => Promise<DeviceLoad[]>;
	/** A device's CPU history and its busiest processes over the last `windowSecs`. */
	readonly readDiagnostics: (
		device: DeviceId,
		windowSecs: number,
		bucketSecs: number
	) => Promise<DiagnosticsResult>;
	/** Updates agent CLIs on a device, on the host or in its workspaces. */
	readonly updateTools: (device: DeviceId, tools: ToolTarget[]) => Promise<ActionResult>;
	/**
	 * Brings each device's `maxd` up to the app's build, one at a time, and
	 * its CLIs too when `tools` is set. `unsigned` says you agreed to send a
	 * `maxd` no release signed, built on this Mac; without it a device that
	 * would get one is skipped. `onProgress` hears each step.
	 */
	readonly updateFleet: (
		devices: DeviceId[],
		tools: boolean,
		unsigned: boolean,
		onProgress: (progress: FleetProgress) => void
	) => Promise<CommandResult>;
	/**
	 * Sets up `maxd` on a new host and links it. It stops at the host key
	 * until `confirmHostKey` answers, and `onProgress` hears each step.
	 */
	readonly setupDevice: (
		host: string,
		secrets: SetupSecret[],
		onProgress: (progress: SetupProgress) => void
	) => Promise<SetupResult>;
	/** Answers a setup's host key question. Nothing runs on the host before. */
	readonly confirmHostKey: (setup: string, ok: boolean) => Promise<CommandResult>;
	/** Sets the SSH hosts a link tries, in order. */
	readonly setRoutes: (link: LinkId, hosts: string[]) => Promise<CommandResult>;
	/**
	 * Answers whether a link that waits on you is the device its hello
	 * named. The same device pins it there, so either link reaches it; a
	 * different one keeps it cut off with a note.
	 */
	readonly answerLinkClaim: (link: LinkId, same: boolean) => Promise<CommandResult>;
	/** Asks again about a link you said is a different device. */
	readonly askLinkClaimAgain: (link: LinkId) => Promise<CommandResult>;
	/** Forgets which device a link reaches, so its next hello counts as its first. */
	readonly forgetLinkIdentity: (link: LinkId) => Promise<CommandResult>;
	/** The addresses, host keys and first sightings the question about a held link shows. */
	readonly linkClaimDetails: (link: LinkId) => Promise<LinkClaimDetailsResult>;
	/** The hosts the Add device wizard offers, from SSH config and the tailnet. */
	readonly listSetupHosts: () => Promise<SetupHost[]>;
	/**
	 * The permissions to run agents that one device holds, or with `device`
	 * null those of every connected device, each once.
	 */
	readonly listPermissions: (device: DeviceId | null) => Promise<PermissionsResult>;
	/**
	 * Writes a permission to its home device and to every device it names,
	 * replacing the one with its ID. Devices it no longer names drop it.
	 */
	readonly grantPermission: (permission: Permission) => Promise<ActionResult>;
	/**
	 * Removes a permission from every device that holds it. The agents it
	 * started keep running.
	 */
	readonly revokePermission: (permission: PermissionId) => Promise<ActionResult>;
	/**
	 * Makes an access key on a device, with `permission` and the new key as
	 * its holder, and writes the permission to the other devices it names.
	 * The outcome gives the key file's path, never the key.
	 */
	readonly mintKey: (
		device: DeviceId,
		name: string,
		permission: Permission
	) => Promise<ActionResult>;
	/**
	 * The access keys one device made, or with `device` null those of every
	 * connected device. Revoking a key's permission removes the key.
	 */
	readonly listKeys: (device: DeviceId | null) => Promise<KeysResult>;
	/** The other devices `device` links to, each with where the link stands. */
	readonly listDeviceLinks: (device: DeviceId) => Promise<DeviceLinksResult>;
	/**
	 * Turns the link between two devices off, which takes each out of the
	 * other, or with `on` allows it again.
	 */
	readonly setDeviceLink: (
		device: DeviceId,
		peer: DeviceId,
		on: boolean
	) => Promise<CommandResult>;
	/** The devices whose `maxd` takes phones, each with what stops a phone reaching it. */
	readonly listPhoneDevices: () => Promise<PhoneDevicesResult>;
	/**
	 * Starts pairing a phone through `through`, for it to reach each of
	 * `devices`, and answers the code for the phone to scan.
	 */
	readonly beginPhonePair: (
		through: DeviceId,
		devices: readonly DeviceId[]
	) => Promise<PhonePairResult>;
	/** Removes a phone from every device that has it. */
	readonly removePhone: (phone: PhoneId) => Promise<CommandResult>;
	/**
	 * Lets a phone do everything but let other keys in, or stops it, on each
	 * connected device that has it, and names the devices it didn't reach.
	 */
	readonly setPhoneEverything: (
		phone: PhoneId,
		everything: boolean
	) => Promise<PhoneEverythingResult>;
	/**
	 * Has each connected device that has a phone with its network access on,
	 * but no sign-in from it, make a new one-time code for the phone's book,
	 * and names the devices that made none.
	 */
	readonly renewPhoneCodes: (phone: PhoneId) => Promise<PhoneCodesResult>;
	/**
	 * Sets the address a phone dials a device at, after the ones the Mac
	 * finds, or with null forgets it.
	 */
	readonly setPhoneAddress: (device: DeviceId, address: string | null) => Promise<CommandResult>;
	/** Ends a phone's sign-in to a device's network access. */
	readonly revokePhoneSession: (session: PhoneSessionId) => Promise<ActionResult>;
	/** This Mac's app key, and how the devices stand with it. */
	readonly appKey: () => Promise<AppKeyResult>;
	/** Has `device` stop trusting an app key. This Mac's own key stays. */
	readonly removeAppKey: (device: DeviceId, publicKey: string) => Promise<ActionResult>;
	/**
	 * Makes a new app key for this Mac. Each device moves to it as the app
	 * next reaches it, and the old key stays until every device has.
	 */
	readonly rotateAppKey: () => Promise<CommandResult>;
	/**
	 * Answers a card a device raised about a program that connected without
	 * a key it trusts: a yes lets it in until it disconnects, or trusts its
	 * key when the card asks that. Only this app answers one, once the
	 * device has its key.
	 */
	readonly answerAdmission: (admission: AdmissionId, allow: boolean) => Promise<ActionResult>;
	/** The limits and depth a new permission starts with. */
	readonly agentDefaults: () => Promise<AgentDefaults>;
	/** Changes the limits and depth new permissions start with. */
	readonly setAgentDefaults: (defaults: AgentDefaults) => Promise<CommandResult>;
	/**
	 * The scheduled tasks one device holds, or with `device` null those of
	 * every connected device.
	 */
	readonly listTasks: (device: DeviceId | null) => Promise<TasksResult>;
	/**
	 * Stores a task on `device`, its home, replacing the one with its ID. A
	 * save that makes a webhook gives its URL this once.
	 */
	readonly saveTask: (device: DeviceId, task: Task) => Promise<ActionResult>;
	/** Turns a task `device` holds on or off, changing nothing else about it. */
	readonly setTaskEnabled: (
		device: DeviceId,
		task: TaskId,
		enabled: boolean
	) => Promise<ActionResult>;
	/** Removes a task. A run in progress goes on. */
	readonly deleteTask: (task: TaskId) => Promise<ActionResult>;
	/** Runs a task now, whatever its trigger. */
	readonly runTaskNow: (task: TaskId) => Promise<ActionResult>;
	/** Turns a task's webhook on or off. */
	readonly setHookOn: (task: TaskId, on: boolean) => Promise<ActionResult>;
	/** Gives a task's webhook a new URL, shown this once. The old one stops working. */
	readonly rotateHookToken: (task: TaskId) => Promise<ActionResult>;
	/** Stores the secret a task's webhook checks signatures with. It never comes back. */
	readonly setHookSecret: (task: TaskId, secret: string) => Promise<ActionResult>;
	/** Picks how `device`'s webhooks are reached. Each hook keeps its token. */
	readonly setHookAddress: (device: DeviceId, address: HookAddress) => Promise<ActionResult>;
	/** A task's recent runs. */
	readonly listRuns: (task: TaskId) => Promise<RunsResult>;
	/** A task's recent webhook requests or GitHub events. */
	readonly listDeliveries: (task: TaskId) => Promise<DeliveriesResult>;
	/** Whether a device's webhooks reach the internet, and where, or what it needs first. */
	readonly funnelState: (device: DeviceId) => Promise<FunnelResult>;
	/**
	 * Sends `device`'s dispatcher a request under `account`, in a new
	 * conversation when `fresh` is set. `refs` are the pills in it, which
	 * go only to a maxd that takes them. Its reply, questions and plan come
	 * through the state.
	 */
	readonly askDispatcher: (
		device: DeviceId,
		text: string,
		account: AccountId,
		fresh: boolean,
		refs?: readonly InlineRef[]
	) => Promise<ActionResult>;
	/** Runs the plan `device`'s dispatcher shows, item by item, as `plan_ran`. */
	readonly confirmPlan: (device: DeviceId, plan: number) => Promise<ActionResult>;
	/** Drops the plan `device`'s dispatcher shows, so nothing in it runs. */
	readonly dismissPlan: (device: DeviceId, plan: number) => Promise<ActionResult>;
	/**
	 * Puts back a change `device`'s dispatcher made at once, by the key on
	 * its card. It's refused once that change can't be undone any more.
	 */
	readonly undoDispatcherAction: (device: DeviceId, action: string) => Promise<ActionResult>;
	/**
	 * Picks, renames, archives or saves one of `device`'s dispatcher chats,
	 * or keeps the message the dispatcher moved out of it there. Delete one
	 * with `deleteAgent`.
	 */
	readonly actOnChat: (
		device: DeviceId,
		agent: AgentId,
		action: ChatAct
	) => Promise<ActionResult>;
	/** A project's PRs on one tab, read on the project's device. */
	readonly listPulls: (project: ProjectId, state: PullListState) => Promise<ActionResult>;
	/** A project's repository's branches, read on the project's device, as `branches`. */
	readonly listBranches: (project: ProjectId) => Promise<ActionResult>;
	/** One PR in full, read on `device`. */
	readonly getPull: (device: DeviceId, repo: string, number: number) => Promise<ActionResult>;
	/** One page of a PR's changed files, from page 1, read on `device`. */
	readonly pullFiles: (
		device: DeviceId,
		repo: string,
		number: number,
		page: number
	) => Promise<ActionResult>;
	/** A PR's conversation, read on `device`. */
	readonly pullComments: (
		device: DeviceId,
		repo: string,
		number: number
	) => Promise<ActionResult>;
	/** A PR's checks on its head, read on `device`. */
	readonly pullChecks: (device: DeviceId, repo: string, number: number) => Promise<ActionResult>;
	/**
	 * Merges or approves a PR as you, on the device holding your GitHub
	 * sign-in. GitHub refuses it when the head isn't `expectSha` anymore.
	 */
	readonly pullAction: (
		repo: string,
		number: number,
		action: PullAction,
		expectSha: string
	) => Promise<ActionResult>;
	/** Has `device` read a repository's PRs again now, for the agents linked to them. */
	readonly pullsDue: (device: DeviceId, repo: string) => Promise<ActionResult>;
	/** Links a PR in the agent's repository to the agent, on its device. */
	readonly linkPull: (agent: AgentId, number: number) => Promise<ActionResult>;
	/** Dismisses a PR's link from the agent, so it isn't linked again. */
	readonly unlinkPull: (agent: AgentId, repo: string, number: number) => Promise<ActionResult>;
	/**
	 * Starts signing in to GitHub on `device`, which then holds the sign-in.
	 * The outcome gives the code to enter. `clientId` is the OAuth app's,
	 * or null for the device's own GitHub App's.
	 */
	readonly beginGithubSignIn: (
		device: DeviceId,
		clientId: string | null
	) => Promise<ActionResult>;
	/** Stops a GitHub sign-in that's waiting for its code. */
	readonly cancelGithubSignIn: (device: DeviceId, signIn: SignInId) => Promise<ActionResult>;
	/**
	 * Asks how a GitHub sign-in stands: done while it waits and once it
	 * signed you in, refused with why once GitHub turned it down.
	 */
	readonly checkGithubSignIn: (device: DeviceId, signIn: SignInId) => Promise<ActionResult>;
	/**
	 * Runs the CLI's own login again for a Claude, Codex or Cursor account
	 * on `device`, the one that holds it. The outcome gives the page to
	 * sign in on. `deviceCode` asks Codex for a code to enter on any device.
	 */
	readonly beginCliLogin: (
		device: DeviceId,
		account: AccountId,
		deviceCode: boolean
	) => Promise<ActionResult>;
	/**
	 * Gives a Claude login the code its sign-in page showed. The webview
	 * drops the code once it's sent.
	 */
	readonly finishCliLogin: (
		device: DeviceId,
		signIn: SignInId,
		code: string
	) => Promise<ActionResult>;
	/** Asks how a CLI login stands. */
	readonly checkCliLogin: (device: DeviceId, signIn: SignInId) => Promise<ActionResult>;
	/** Stops a CLI login that waits for you. */
	readonly cancelCliLogin: (device: DeviceId, signIn: SignInId) => Promise<ActionResult>;
	/** Looks again at what `device` can host, such as after you install Xcode. */
	readonly checkTestHosts: (device: DeviceId) => Promise<ActionResult>;
	/** Builds a test host on `device` that's off or failed. */
	readonly buildTestHost: (device: DeviceId, host: TestHostId) => Promise<ActionResult>;
	/** Takes a test host on `device` back to a clean state, ending its lease. */
	readonly resetTestHost: (device: DeviceId, host: TestHostId) => Promise<ActionResult>;
	/** Ends a lease on `device` now. */
	readonly endLease: (device: DeviceId, lease: LeaseId) => Promise<ActionResult>;
	/** A screenshot of a ready lease's machine on `device`, as `lease_screen`. */
	readonly readLeaseScreen: (device: DeviceId, lease: LeaseId) => Promise<ActionResult>;
	/**
	 * Calls `onUpdate` with a workflow run's reset, then with each patch,
	 * until unsubscribed. Every open starts with a reset of its own.
	 */
	readonly openWorkflow: (
		run: RunKey,
		onUpdate: (update: WorkflowUpdate) => void
	) => Promise<Unsubscribe>;
	/** More about one node of a run, asked of the run's device. */
	readonly nodeDetail: (run: RunKey, node: string) => Promise<NodeDetailResult>;
	/** Adds a source to the skill library `device` holds. */
	readonly addSkillSource: (
		device: DeviceId,
		name: string,
		origin: SourceKind,
		defaultRef: string | null,
		trust: SourceTrust
	) => Promise<ActionResult>;
	/** Fetches a source of the library `device` holds again. */
	readonly fetchSkillSource: (device: DeviceId, source: SkillSourceId) => Promise<ActionResult>;
	/** Removes a source from the library `device` holds. */
	readonly removeSkillSource: (device: DeviceId, source: SkillSourceId) => Promise<ActionResult>;
	/** Sets a project's skills on the device that owns its lock. */
	readonly setProjectSkills: (
		device: DeviceId,
		project: ProjectId,
		lock: SkillLock
	) => Promise<ActionResult>;
	/** Puts a project's locked skills into each of its checkouts on `device`. */
	readonly applySkills: (device: DeviceId, project: ProjectId) => Promise<ActionResult>;
	/** The skills each CLI sees in a place on `device`, as `skills`. */
	readonly scanSkills: (device: DeviceId, place: AgentPlace) => Promise<ActionResult>;
	/** The trees of a lock that `device`'s store lacks, as `skill_trees`. */
	readonly skillTreesNeeded: (device: DeviceId, lock: SkillLock) => Promise<ActionResult>;
	/**
	 * Approves or rejects a skill proposal, pushing its source after an
	 * approve when `push` is set. A third party's skill is approved as a
	 * fork into the personal source, named by `fork`. An approve carries
	 * what the owner read as `seen`, and is refused if that no longer holds.
	 */
	readonly answerSkillProposal: (
		proposal: SkillProposalId,
		action: ProposalAction,
		push: boolean,
		fork?: string,
		seen?: ProposalSeen
	) => Promise<ActionResult>;
	/** What a skill proposal changes against its base, as `skill_proposal_diff`. */
	readonly skillProposalDiff: (proposal: SkillProposalId) => Promise<ActionResult>;
	/** What tidying `device`'s global skills would do, as `skills_globals_plan`. */
	readonly planGlobalSkills: (device: DeviceId) => Promise<ActionResult>;
	/**
	 * Tidies `device`'s global skills as the plan with this hash said,
	 * keeping the copies at `picks` of skills whose copies differ.
	 */
	readonly applyGlobalSkills: (
		device: DeviceId,
		planHash: string,
		picks: readonly string[]
	) => Promise<ActionResult>;
	/** Puts back what the last tidy on `device` moved. */
	readonly undoGlobalSkills: (device: DeviceId) => Promise<ActionResult>;
	/**
	 * Makes `device` the one that holds the skill library, moving its
	 * sources from the device that holds it now.
	 */
	readonly moveSkillLibrary: (device: DeviceId) => Promise<ActionResult>;
	/** Pushes a source of the library `device` holds to its remote. */
	readonly pushSkillSource: (device: DeviceId, source: SkillSourceId) => Promise<ActionResult>;
	/** Has `device` read its host spec and check itself against it again. */
	readonly checkHost: (device: DeviceId) => Promise<ActionResult>;
	/** Fixes the host report's `items` that `device`'s `maxd` can fix itself. */
	readonly applyHost: (device: DeviceId, items: readonly string[]) => Promise<ActionResult>;
	/** Installs a tool on `device`, at `version` or else the latest. */
	readonly installTool: (
		device: DeviceId,
		place: ToolPlace,
		tool: ToolKind,
		version?: string
	) => Promise<ActionResult>;
	/** Removes a tool from `device`. */
	readonly removeTool: (
		device: DeviceId,
		place: ToolPlace,
		tool: ToolKind
	) => Promise<ActionResult>;
	/** A container workspace's snapshots, as `snapshots`. */
	readonly listSnapshots: (workspace: WorkspaceId) => Promise<ActionResult>;
	/**
	 * Takes a container workspace back to a snapshot, or makes a new
	 * workspace from it when `asNew` is set. A roll back over unsaved work
	 * goes ahead only with `discardUnsaved`.
	 */
	readonly restoreSnapshot: (
		workspace: WorkspaceId,
		snapshot: string,
		asNew: boolean,
		discardUnsaved: boolean
	) => Promise<ActionResult>;
	/** Starts a move of `device`'s container `instance` into MaxHQ. */
	readonly planMove: (device: DeviceId, instance: string) => Promise<ActionResult>;
	/** Runs one step of a move. */
	readonly runMoveStep: (moving: MoveId, step: MoveStep) => Promise<ActionResult>;
	/** Switches a move back to the old container while it's kept. */
	readonly moveBack: (moving: MoveId) => Promise<ActionResult>;
	/** Deletes a moved workspace's old container now. */
	readonly retireMove: (moving: MoveId) => Promise<ActionResult>;
	/**
	 * Installs an agent from `device`'s ACP registry. A binary the registry
	 * gives no checksum for needs `confirmUnchecked`.
	 */
	readonly installAcpAgent: (
		device: DeviceId,
		agent: string,
		version: string,
		place: ToolPlace,
		confirmUnchecked: boolean
	) => Promise<ActionResult>;
	/** Accepts a licence an install on `device` waits on, as read. */
	readonly acceptLicence: (
		device: DeviceId,
		licence: string,
		textSha256: string
	) => Promise<ActionResult>;
	/** Present only on the sample data. */
	readonly mock?: MockControls;
};

/** Which source a build uses, from the switch, the build mode and whether it runs inside Tauri. */
export const pickSource = (
	setting: string | undefined,
	dev: boolean,
	inTauri: boolean
): DataSource["kind"] => {
	if (setting === "mock" || setting === "tauri") return setting;
	return dev || !inTauri ? "mock" : "tauri";
};
