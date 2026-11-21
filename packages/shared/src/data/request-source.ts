// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The part of a backend source that's the same on every platform. Everything
// that answers once is one request, which comes back with a response of its
// kind, and timelines and workflow runs each share one stream per window.
// Each platform brings how a request gets there and how a stream's batches
// come back.

import { t } from "../i18n";

import {
	MAX_INLINE_ATTACHMENT,
	type AgentId,
	type CommandError,
	type Request,
	type Response,
	type RunKey,
	type TimelineMessage,
	type TimelineUpdate,
	type WorkflowMessage,
	type WorkflowUpdate,
} from "../bindings";
import { folderCalls } from "../folders/requests";
import { noticeCalls } from "../notices/requests";
import { cursorRuleCalls } from "../settings/cursor-rules";
import { resetCreditCalls } from "../status/reset-credits";
import { terminalCalls } from "../terminal/requests";
import { sessionCalls } from "../terminal/session";
import { runName } from "../workflow/select";
import type { ActionResult, CommandResult, DataSource, GitItemRef, Unsubscribe } from "./source";

/** The most a paste can carry: the protocol's limit on inline bytes. */
export const MAX_PASTE = MAX_INLINE_ATTACHMENT;

/** What a paste over the limit comes back with. It never reaches the device. */
const TOO_LARGE: ActionResult = {
	status: "error",
	error: {
		kind: "failed",
		get message() {
			return t("A paste can be at most {{MAX_PASTE____1024___1024}} MiB", {
				MAX_PASTE____1024___1024: MAX_PASTE / (1024 * 1024),
			});
		},
	},
};

/** A value, or why there's none. */
export type Result<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

/** Sends one request to the backend, as a platform does. */
export type Send = (request: Request) => Promise<Result<Response>>;

type Kind = Request["kind"];

/** What a request of kind `K` answers with, which is null when it's nothing. */
type ValueOf<K extends Kind> = Extract<Response, { kind: K }> extends { value: infer V } ? V : null;

/** Sends one request and gives the value of its response. */
export type Ask = <K extends Kind>(
	request: Extract<Request, { kind: K }>
) => Promise<Result<ValueOf<K>>>;

/** Asks over `send`, checking each response is of its request's kind. */
export const asker =
	(send: Send): Ask =>
	async <K extends Kind>(request: Extract<Request, { kind: K }>) => {
		const result = await send(request);
		if (result.status === "error") return result;
		const response = result.data;
		if (response.kind !== request.kind) {
			throw new Error(
				t("A {{request_kind}} request came back as {{response_kind}}", {
					request_kind: request.kind,
					response_kind: response.kind,
				})
			);
		}
		const value = "value" in response ? response.value : null;
		return { status: "ok", data: value as ValueOf<K> };
	};

/** Throws a failed request's error, for a call that resolves with its value. */
export const orThrow = <T>(result: Result<T>) => {
	if (result.status === "error") {
		throw new Error(describe(result.error), { cause: result.error });
	}
	return result.data;
};

/** A failed command's error in words, for an open that has to throw. */
export const describe = (error: CommandError): string => {
	switch (error.kind) {
		case "not_found":
			return t("{{error_what}} isn't there anymore", { error_what: error.what });
		case "not_connected":
			return t("{{error_device}} isn't connected", { error_device: error.device });
		case "failed":
			return error.message;
	}
};

/** Bytes at a time through `String.fromCharCode`, which takes them as arguments. */
const CHUNK = 0x8000;

/** Encodes bytes as base64 without building one argument per byte. */
export const toBase64 = (bytes: Uint8Array): string => {
	let binary = "";
	for (let at = 0; at < bytes.length; at += CHUNK) {
		binary += String.fromCharCode(...bytes.subarray(at, at + CHUNK));
	}
	return btoa(binary);
};

/**
 * Sends each call once the one before it has answered. Requests run side by
 * side in the backend, and what a window shows has to land in the order the
 * page said it.
 */
const inOrder = <A extends unknown[]>(send: (...args: A) => Promise<void>) => {
	let last = Promise.resolve();
	return (...args: A): Promise<void> => {
		const next = last.then(() => send(...args));
		last = next.catch(() => undefined);
		return next;
	};
};

/** How one kind of stream opens over a window's one channel. */
type Stream<K, M, U, C> = {
	/** Makes the window's channel, which hands each batch to `deliver`. */
	readonly channel: (deliver: (batch: M[]) => void) => C;
	/** Opens `key` on the backend, with updates coming down `channel`. */
	readonly open: (key: K, channel: C) => Promise<CommandResult>;
	/** Takes one open of `key` back. */
	readonly close: (key: K) => void;
	/** Names a key, so updates find whoever opened it. */
	readonly name: (key: K) => string;
	/** The key's name, the update a message carries, and whether it just happened. */
	readonly split: (message: M) => readonly [string, U, boolean];
	/** Whether an update replaces everything before it. */
	readonly isReset: (update: U) => boolean;
};

/**
 * Opens streams over the window's one channel for their kind and hands each
 * update to whoever opened that key. Timelines and workflow runs each get one.
 */
const streams = <K, M, U, C>(stream: Stream<K, M, U, C>) => {
	type Listener = (update: U, live: boolean) => void;
	const listeners = new Map<string, Set<Listener>>();
	let channel: C | undefined;

	const deliver = (batch: M[]) => {
		for (const message of batch) {
			const [name, update, live] = stream.split(message);
			for (const listener of [...(listeners.get(name) ?? [])]) listener(update, live);
		}
	};

	// The channel is made on first use, so a source that never opens one
	// never asks the runtime for it.
	const channelOf = () => {
		channel ??= stream.channel(deliver);
		return channel;
	};

	const remove = (name: string, listener: Listener) => {
		const set = listeners.get(name);
		set?.delete(listener);
		if (set?.size === 0) listeners.delete(name);
	};

	return async (key: K, onUpdate: Listener): Promise<Unsubscribe> => {
		// Each open starts with a reset of its own. Until it arrives, the
		// changes meant for an earlier open of the same key get dropped,
		// since there's nothing yet to apply them to.
		let started = false;
		const listener: Listener = (update, live) => {
			if (stream.isReset(update)) started = true;
			if (started) onUpdate(update, live);
		};
		const name = stream.name(key);
		const set = listeners.get(name) ?? new Set<Listener>();
		set.add(listener);
		listeners.set(name, set);

		const opened = await stream.open(key, channelOf());
		if (opened.status === "error") {
			remove(name, listener);
			throw new Error(describe(opened.error), { cause: opened.error });
		}
		let open = true;
		return () => {
			if (!open) return;
			open = false;
			remove(name, listener);
			stream.close(key);
		};
	};
};

/** Opens timelines by agent over one channel of type `C`. */
export const timelines = <C>(
	ask: Ask,
	channel: (deliver: (batch: TimelineMessage[]) => void) => C,
	open: (agent: AgentId, channel: C) => Promise<CommandResult>
) =>
	streams<AgentId, TimelineMessage, TimelineUpdate, C>({
		channel,
		open,
		close: (agent) => void ask({ kind: "close_timeline", agent }),
		name: (agent) => agent,
		split: ({ agent, update, live }) => [agent, update, live],
		isReset: (update) => update.type === "reset",
	});

/** Opens workflow runs by key over one channel of type `C`. */
export const workflows = <C>(
	ask: Ask,
	channel: (deliver: (batch: WorkflowMessage[]) => void) => C,
	open: (run: RunKey, channel: C) => Promise<CommandResult>
) =>
	streams<RunKey, WorkflowMessage, WorkflowUpdate, C>({
		channel,
		open,
		close: (run) => void ask({ kind: "close_workflow", run }),
		name: runName,
		split: ({ run, update }) => [runName(run), update, false],
		isReset: (update) => update.kind === "reset",
	});

/** The calls a platform brings itself: events, streams and transfers. */
export type PlatformCalls =
	| "kind"
	| "subscribe"
	| "onFocusAgent"
	| "openTimeline"
	| "openWorkflow"
	| "copyProject"
	| "upload"
	| "uploadPasted"
	| "attachFromAgent"
	| "download"
	| "media"
	| "updateFleet"
	| "setupDevice";

/** Every call that's one request, over `ask`. */
export const requestCalls = (ask: Ask): Omit<DataSource, PlatformCalls> => ({
	...terminalCalls(ask),
	...sessionCalls(ask),
	...folderCalls(ask),
	...noticeCalls(ask),
	...cursorRuleCalls(ask),
	...resetCreditCalls(ask),
	appInfo: async () => orThrow(await ask({ kind: "app_info" })),
	snapshot: async () => orThrow(await ask({ kind: "view_snapshot" })),
	markSeen: (agent) => ask({ kind: "mark_seen", agent }),
	refreshUsage: (account) => ask({ kind: "refresh_usage", account }),
	refreshLinks: (link) => ask({ kind: "refresh_links", link }),
	setShown: inOrder(async (shown) => {
		orThrow(await ask({ kind: "set_shown", shown }));
	}),
	nodeDetail: (run, node) => ask({ kind: "node_detail", run, node }),
	loadOlder: (agent, before) => ask({ kind: "load_older", agent, before }),
	startAgent: (spec) => ask({ kind: "start_agent", spec }),
	sendInput: (agent, input, mode) => ask({ kind: "send_input", agent, input, mode }),
	cancelQueued: (agent, item) => ask({ kind: "cancel_queued", agent, item }),
	interrupt: (agent) => ask({ kind: "interrupt", agent }),
	setPermission: (agent, mode) => ask({ kind: "set_permission", agent, mode }),
	setModel: (agent, model, account) =>
		ask({ kind: "set_model", agent, model, account: account ?? null }),
	setEffort: (agent, effort) => ask({ kind: "set_effort", agent, effort }),
	stopAgent: (agent) => ask({ kind: "stop_agent", agent }),
	deleteAgent: (agent) => ask({ kind: "delete_agent", agent }),
	moveAgent: (device, agent, from, to, stop) =>
		ask({ kind: "move_agent", device, agent, from, to, stop }),
	renameAgent: (agent, title) => ask({ kind: "rename_agent", agent, title }),
	setInbox: async (agents, change) => orThrow(await ask({ kind: "set_inbox", agents, change })),
	continueHere: (agent) => ask({ kind: "continue_here", agent }),
	takeOver: (agent, mode) => ask({ kind: "take_over", agent, mode }),
	findPaths: async (agent, query) => orThrow(await ask({ kind: "find_paths", agent, query })),
	findGitItems: async (agent, query, repo) => {
		const search = orThrow(
			await ask({ kind: "find_git_items", agent, query, repo: repo ?? null })
		);
		const items = search.items.filter((item): item is GitItemRef => item.type === "git_item");
		return { ...search, items };
	},
	excerptAgent: async (agent, budget) =>
		orThrow(await ask({ kind: "excerpt_agent", agent, budget })),
	findInTimeline: async (agent, query, at) =>
		orThrow(await ask({ kind: "find_in_timeline", agent, query, at })),
	answer: (event, answer, refs = []) =>
		ask({ kind: "answer", event, answer, ...(refs.length > 0 ? { refs: [...refs] } : {}) }),
	stageAttachments: async (device, paths) =>
		orThrow(await ask({ kind: "stage_offered", device, paths })),
	stageFound: async (agent, paths) => orThrow(await ask({ kind: "stage_found", agent, paths })),
	stagePasted: async (device, name, mime, bytes) =>
		bytes.length > MAX_PASTE
			? TOO_LARGE
			: ask({ kind: "stage_pasted", device, name, mime, data: toBase64(bytes) }),
	setDeviceSettings: (settings) => ask({ kind: "set_device_settings", settings }),
	createWorkspace: (spec) => ask({ kind: "create_workspace", spec }),
	startWorkspace: (workspace) => ask({ kind: "start_workspace", workspace }),
	stopWorkspace: (workspace) => ask({ kind: "stop_workspace", workspace }),
	deleteWorkspace: (workspace, discardUnpushed) =>
		ask({ kind: "delete_workspace", workspace, discard_unpushed: discardUnpushed }),
	listIgnoredFiles: async (workspace) =>
		orThrow(await ask({ kind: "list_ignored_files", workspace })),
	cancelClone: (workspace) => ask({ kind: "clone_cancel", workspace }),
	retryClone: (workspace) => ask({ kind: "clone_retry", workspace }),
	removeClone: (workspace) => ask({ kind: "clone_remove", workspace }),
	sshInclude: async (workspace) => orThrow(await ask({ kind: "ssh_include", workspace })).state,
	setupSshInclude: async (workspace, add) =>
		orThrow(await ask({ kind: "setup_ssh_include", workspace, add })).state,
	listEditors: async () => orThrow(await ask({ kind: "list_editors" })),
	openInEditor: (editor, workspace, path, line, column) =>
		ask({
			kind: "open_in_editor",
			editor,
			workspace,
			path: path ?? null,
			line: line ?? null,
			column: column ?? null,
		}),
	setProjectSettings: (settings) => ask({ kind: "set_project_settings", settings }),
	setProjectHidden: (project, hidden) => ask({ kind: "set_project_hidden", project, hidden }),
	removeCommandRule: (device, project, rule) =>
		ask({ kind: "remove_command_rule", device, project, rule }),
	removeAllowedEdit: (device, project, kind) =>
		ask({ kind: "remove_allowed_edit", device, project, edit: kind }),
	windDown: (device, windowSecs) => ask({ kind: "wind_down", device, window_secs: windowSecs }),
	restartMaxd: (link) => ask({ kind: "restart_maxd", link }),
	removeDevice: (link) => ask({ kind: "remove_device", link }),
	startLocalMaxd: () => ask({ kind: "start_local_maxd" }),
	uninstallLocalMaxd: (deleteData) =>
		ask({ kind: "uninstall_local_maxd", delete_data: deleteData }),
	// The token goes straight to the backend, and nothing here keeps it.
	storeClaudeToken: async (devices, origin, label, token) =>
		orThrow(await ask({ kind: "store_claude_token", devices, origin, label, token })),
	storeCursorKey: async (devices, label, token) =>
		orThrow(await ask({ kind: "store_cursor_key", devices, label, token })),
	signInChatGpt: (device) => ask({ kind: "sign_in_chatgpt", device }),
	setGithubApp: async (devices, appId, keyPath) =>
		orThrow(await ask({ kind: "set_github_app", devices, app_id: appId, key_path: keyPath })),
	checkGithubApp: (device) => ask({ kind: "check_github_app", device }),
	setApnsKey: async (devices, keyId, teamId, keyPath) =>
		orThrow(
			await ask({
				kind: "set_apns_key",
				devices,
				key_id: keyId,
				team_id: teamId,
				key_path: keyPath,
			})
		),
	forgetCredential: (device, credential) =>
		ask({ kind: "forget_credential", device, credential }),
	// The values go straight to the backend, and nothing here keeps them.
	setAcpAgent: (device, agent, env) => ask({ kind: "set_acp_agent", device, agent, env }),
	deleteAcpAgent: (device, id) => ask({ kind: "delete_acp_agent", device, id }),
	modelAction: (device, action) => ask({ kind: "model_action", device, action }),
	gitIdentity: async () => orThrow(await ask({ kind: "git_identity" })),
	cancelTransfer: async (transfer) => {
		orThrow(await ask({ kind: "cancel_transfer", transfer }));
	},
	openForward: (port) => ask({ kind: "open_forward", port }),
	closeForward: async (forward) => {
		orThrow(await ask({ kind: "close_forward", forward }));
	},
	openLocalhost: async (port) => {
		orThrow(await ask({ kind: "open_localhost", port }));
	},
	listProcesses: (agent) => ask({ kind: "list_processes", agent }),
	signalProcess: (agent, pid, start, signal) =>
		ask({ kind: "signal_process", agent, pid, start, signal }),
	readNote: (device, project, note, version) =>
		ask({ kind: "read_note", device, project, note, version }),
	saveNote: (device, project, note, save) =>
		ask({ kind: "save_note", device, project, note, ...save }),
	noteHistory: (device, project, note) => ask({ kind: "note_history", device, project, note }),
	readLoad: async (devices) => orThrow(await ask({ kind: "read_load", devices })),
	readDiagnostics: async (device, windowSecs, bucketSecs) => {
		const result = await ask({
			kind: "read_diagnostics",
			device,
			window_secs: windowSecs,
			bucket_secs: bucketSecs,
		});
		if (result.status === "error") return result;
		const outcome = result.data;
		if (outcome.status !== "diagnostics") {
			return {
				status: "error",
				error: { kind: "failed", message: t("The device answered with something else") },
			};
		}
		const { buckets, top, containers, error } = outcome;
		return { status: "ok", data: { buckets, top, containers, error } };
	},
	updateTools: (device, tools) => ask({ kind: "update_tools", device, tools }),
	confirmHostKey: (setup, ok) => ask({ kind: "confirm_host_key", setup, ok }),
	setRoutes: (link, hosts) => ask({ kind: "set_routes", link, hosts }),
	answerLinkClaim: (link, same) => ask({ kind: "answer_link_claim", link, same }),
	askLinkClaimAgain: (link) => ask({ kind: "ask_link_claim_again", link }),
	forgetLinkIdentity: (link) => ask({ kind: "forget_link_identity", link }),
	linkClaimDetails: (link) => ask({ kind: "link_claim_details", link }),
	listSetupHosts: async () => orThrow(await ask({ kind: "list_setup_hosts" })),
	listPermissions: (device) => ask({ kind: "list_permissions", device }),
	grantPermission: (permission) => ask({ kind: "grant_permission", permission }),
	revokePermission: (permission) => ask({ kind: "revoke_permission", permission }),
	mintKey: (device, name, permission) => ask({ kind: "mint_key", device, name, permission }),
	listKeys: (device) => ask({ kind: "list_keys", device }),
	listDeviceLinks: (device) => ask({ kind: "list_device_links", device }),
	setDeviceLink: (device, peer, on) => ask({ kind: "set_device_link", device, peer, on }),
	listPhoneDevices: () => ask({ kind: "list_phone_devices" }),
	beginPhonePair: (through, devices) =>
		ask({ kind: "begin_phone_pair", through, devices: [...devices] }),
	removePhone: (phone) => ask({ kind: "remove_phone", phone }),
	setPhoneEverything: (phone, everything) =>
		ask({ kind: "set_phone_everything", phone, everything }),
	renewPhoneCodes: (phone) => ask({ kind: "renew_phone_codes", phone }),
	setPhoneAddress: (device, address) => ask({ kind: "set_phone_address", device, address }),
	revokePhoneSession: (session) => ask({ kind: "revoke_phone_session", session }),
	appKey: () => ask({ kind: "app_key" }),
	removeAppKey: (device, publicKey) =>
		ask({ kind: "remove_app_key", device, public_key: publicKey }),
	rotateAppKey: () => ask({ kind: "rotate_app_key" }),
	answerAdmission: (admission, allow) => ask({ kind: "answer_admission", admission, allow }),
	agentDefaults: async () => orThrow(await ask({ kind: "agent_defaults" })),
	setAgentDefaults: (defaults) => ask({ kind: "set_agent_defaults", defaults }),
	listTasks: (device) => ask({ kind: "list_tasks", device }),
	// A save that makes a webhook, and a rotate, answer with its URL. It
	// goes back to the caller alone, and nothing here keeps it.
	saveTask: (device, task) => ask({ kind: "save_task", device, task }),
	setTaskEnabled: (device, task, enabled) =>
		ask({ kind: "set_task_enabled", device, task, enabled }),
	deleteTask: (task) => ask({ kind: "delete_task", task }),
	runTaskNow: (task) => ask({ kind: "run_task_now", task }),
	setHookOn: (task, on) => ask({ kind: "set_hook_on", task, on }),
	setHookAddress: (device, address) => ask({ kind: "set_hook_address", device, address }),
	rotateHookToken: (task) => ask({ kind: "rotate_hook_token", task }),
	// The secret goes straight to the backend, and nothing here keeps it.
	setHookSecret: (task, secret) => ask({ kind: "set_hook_secret", task, secret }),
	listRuns: (task) => ask({ kind: "list_runs", task }),
	listDeliveries: (task) => ask({ kind: "list_deliveries", task }),
	funnelState: (device) => ask({ kind: "funnel_state", device }),
	askDispatcher: (device, text, account, fresh, refs = []) =>
		ask({
			kind: "ask_dispatcher",
			device,
			text,
			account,
			new: fresh,
			...(refs.length > 0 ? { refs: [...refs] } : {}),
		}),
	confirmPlan: (device, plan) => ask({ kind: "confirm_plan", device, plan }),
	dismissPlan: (device, plan) => ask({ kind: "dismiss_plan", device, plan }),
	undoDispatcherAction: (device, action) =>
		ask({ kind: "undo_dispatcher_action", device, action }),
	actOnChat: (device, agent, action) => ask({ kind: "act_on_chat", device, agent, action }),
	listPulls: (project, state) => ask({ kind: "list_pulls", project, state }),
	listBranches: (project) => ask({ kind: "list_branches", project }),
	getPull: (device, repo, number) => ask({ kind: "get_pull", device, repo, number }),
	pullFiles: (device, repo, number, page) =>
		ask({ kind: "pull_files", device, repo, number, page }),
	pullComments: (device, repo, number) => ask({ kind: "pull_comments", device, repo, number }),
	pullChecks: (device, repo, number) => ask({ kind: "pull_checks", device, repo, number }),
	pullAction: (repo, number, action, expectSha) =>
		ask({ kind: "pull_action", repo, number, action, expect_sha: expectSha }),
	pullsDue: (device, repo) => ask({ kind: "pulls_due", device, repo }),
	linkPull: (agent, number) => ask({ kind: "link_pull", agent, number }),
	unlinkPull: (agent, repo, number) => ask({ kind: "unlink_pull", agent, repo, number }),
	beginGithubSignIn: (device, clientId) =>
		ask({ kind: "begin_github_sign_in", device, client_id: clientId }),
	cancelGithubSignIn: (device, signIn) =>
		ask({ kind: "cancel_github_sign_in", device, sign_in: signIn }),
	checkGithubSignIn: (device, signIn) =>
		ask({ kind: "check_github_sign_in", device, sign_in: signIn }),
	beginCliLogin: (device, account, deviceCode) =>
		ask({ kind: "begin_cli_login", device, account, device_code: deviceCode }),
	// The code goes straight to the backend, and nothing here keeps it.
	finishCliLogin: (device, signIn, code) =>
		ask({ kind: "finish_cli_login", device, sign_in: signIn, code }),
	checkCliLogin: (device, signIn) => ask({ kind: "check_cli_login", device, sign_in: signIn }),
	cancelCliLogin: (device, signIn) => ask({ kind: "cancel_cli_login", device, sign_in: signIn }),
	checkTestHosts: (device) => ask({ kind: "check_test_hosts", device }),
	buildTestHost: (device, host) => ask({ kind: "build_test_host", device, host }),
	resetTestHost: (device, host) => ask({ kind: "reset_test_host", device, host }),
	endLease: (device, lease) => ask({ kind: "end_lease", device, lease }),
	readLeaseScreen: (device, lease) => ask({ kind: "read_lease_screen", device, lease }),
	addSkillSource: (device, name, origin, defaultRef, trust) =>
		ask({ kind: "skill_source_add", device, name, origin, default_ref: defaultRef, trust }),
	fetchSkillSource: (device, source) => ask({ kind: "skill_source_fetch", device, source }),
	removeSkillSource: (device, source) => ask({ kind: "skill_source_remove", device, source }),
	setProjectSkills: (device, project, lock) =>
		ask({ kind: "skills_set_project", device, project, lock }),
	applySkills: (device, project) => ask({ kind: "skills_apply", device, project }),
	scanSkills: (device, place) => ask({ kind: "skills_scan", device, place }),
	skillTreesNeeded: (device, lock) => ask({ kind: "skills_trees_needed", device, lock }),
	answerSkillProposal: (proposal, action, push, fork, seen) =>
		ask({
			kind: "skill_proposal_action",
			proposal,
			action,
			push,
			fork: fork ?? null,
			seen: seen ?? null,
		}),
	skillProposalDiff: (proposal) => ask({ kind: "skill_proposal_diff", proposal }),
	planGlobalSkills: (device) => ask({ kind: "skills_globals_plan", device }),
	applyGlobalSkills: (device, planHash, picks) =>
		ask({ kind: "skills_globals_apply", device, plan_hash: planHash, picks: [...picks] }),
	undoGlobalSkills: (device) => ask({ kind: "skills_globals_undo", device }),
	moveSkillLibrary: (device) => ask({ kind: "move_skill_library", device }),
	pushSkillSource: (device, source) => ask({ kind: "skill_source_push", device, source }),
	checkHost: (device) => ask({ kind: "check_host", device }),
	applyHost: (device, items) => ask({ kind: "apply_host", device, items: [...items] }),
	installTool: (device, place, tool, version) =>
		ask({ kind: "install_tool", device, place, tool, version: version ?? null }),
	removeTool: (device, place, tool) => ask({ kind: "remove_tool", device, place, tool }),
	listSnapshots: (workspace) => ask({ kind: "list_snapshots", workspace }),
	restoreSnapshot: (workspace, snapshot, asNew, discardUnsaved) =>
		ask({
			kind: "restore_snapshot",
			workspace,
			snapshot,
			as_new: asNew,
			discard_unsaved: discardUnsaved,
		}),
	planMove: (device, instance) => ask({ kind: "plan_move", device, instance }),
	runMoveStep: (moving, step) => ask({ kind: "run_move_step", moving, step }),
	moveBack: (moving) => ask({ kind: "move_back", moving }),
	retireMove: (moving) => ask({ kind: "retire_move", moving }),
	installAcpAgent: (device, agent, version, place, confirmUnchecked) =>
		ask({
			kind: "install_acp_agent",
			device,
			agent,
			version,
			place,
			confirm_unchecked: confirmUnchecked,
		}),
	acceptLicence: (device, licence, textSha256) =>
		ask({ kind: "accept_licence", device, licence, text_sha256: textSha256 }),
});
