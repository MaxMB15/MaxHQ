// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The requests each backend source sends, as tables a platform's tests run
// over its own transport: each call, and the one request it should send.

import type {
	AcpAgent,
	AgentSpec,
	AttentionAnswer,
	DeviceSettings,
	InlineRef,
	MoveTo,
	Limits,
	Permission,
	ProjectSettings,
	ProposalSeen,
	Request,
	SkillLock,
	SourceKind,
	Task,
	TimelineUpdate,
	ToolKind,
	ToolPlace,
	UserInput,
	WorkspaceSpec,
} from "../bindings";
import type { DataSource } from "../data/source";

export const OK = { status: "ok", data: null } as const;
export const DONE = { status: "ok", data: { status: "done" } } as const;

/** The backend's response to a request of `kind`, with `value` if it has one. */
export const answered = (kind: Request["kind"], value?: unknown) => ({
	status: "ok",
	data: value === undefined ? { kind } : { kind, value },
});

export const SPEC: AgentSpec = {
	vendor: "claude",
	account: "account-1",
	place: { type: "folder", path: "/home/dev/code" },
	model: null,
	permission: null,
	title: null,
	first: null,
	branch: null,
};

const MOVE_TO: MoveTo = {
	device: "device-3",
	account: "account-1",
	model: null,
	place: { type: "workspace", workspace: "workspace-1" },
};

export const INPUT: UserInput = { text: "hello", attachments: [] };
export const ANSWER: AttentionAnswer = { type: "option", id: "allow" };
export const AGENT_PILL: InlineRef = {
	type: "agent",
	agent: "agent-2",
	title: "Release notes",
	vendor: "claude",
	device: "device-1",
	excerpt: null,
};
export const SETTINGS: DeviceSettings = {
	device: "device-1",
	default_permission: "ask",
	workspace_network: null,
	workspace_pool: null,
};
export const HOST: ToolPlace = { type: "host" };
export const NODE: ToolKind = { type: "helper", tool: "node" };
export const WORKSPACE: WorkspaceSpec = {
	project: "project-1",
	device: "device-1",
	name: null,
	start_at_boot: false,
};
export const PROJECT: ProjectSettings = {
	project: "project-1",
	device: "device-1",
	resume_on_restart: true,
	ignored_ports: [5432],
	git_author: null,
};

export const NO_LIMITS: Limits = {
	max_started: null,
	max_running: null,
	usage_cap_percent: null,
	expires_at: null,
};
export const ACP_AGENT: AcpAgent = {
	id: "acp-1",
	device: "device-1",
	name: "Agent",
	program: "agent",
	args: ["acp"],
	env: ["API_KEY"],
};
export const PERMISSION: Permission = {
	id: "permission-1",
	holder: { kind: "agent", agent: "agent-1" },
	home: "device-1",
	devices: ["device-1"],
	vendors: ["claude"],
	accounts: null,
	places: null,
	depth: 3,
	limits: NO_LIMITS,
};

export const TASK: Task = {
	id: "task-1",
	name: "Triage",
	prompt: "Look at the new issues",
	trigger: { type: "every", minutes: 15 },
	target: { type: "agent", device: "device-1", agent: "agent-1" },
	enabled: true,
	permission: "permission-1",
	made_by: { type: "user" },
	next_run: null,
	last_run: null,
};

export const ORIGIN: SourceKind = { kind: "git", url: "https://example.com/example/agent-skills" };
export const LOCK: SkillLock = { skills: [], committed: false };

const SEEN: ProposalSeen = { tree: "tree-1", base: null };

export const reset = (seq: number): TimelineUpdate => ({
	type: "reset",
	seq,
	items: [],
	has_older: false,
});
export const events = (seq: number): TimelineUpdate => ({ type: "events", seq, events: [] });

export type Case = [name: string, call: (source: DataSource) => Promise<unknown>, request: Request];

/** The requests that answer with what came of an action. */
export const ACTIONS: Case[] = [
	["startAgent", (s) => s.startAgent(SPEC), { kind: "start_agent", spec: SPEC }],
	[
		"sendInput",
		(s) => s.sendInput("agent-1", INPUT, "steer"),
		{ kind: "send_input", agent: "agent-1", input: INPUT, mode: "steer" },
	],
	[
		"cancelQueued",
		(s) => s.cancelQueued("agent-1", "item-1"),
		{ kind: "cancel_queued", agent: "agent-1", item: "item-1" },
	],
	["interrupt", (s) => s.interrupt("agent-1"), { kind: "interrupt", agent: "agent-1" }],
	[
		"setPermission",
		(s) => s.setPermission("agent-1", "accept_edits"),
		{ kind: "set_permission", agent: "agent-1", mode: "accept_edits" },
	],
	[
		"setModel",
		(s) => s.setModel("agent-1", "opus"),
		{ kind: "set_model", agent: "agent-1", model: "opus", account: null },
	],
	[
		"setModel to another account",
		(s) => s.setModel("agent-1", "gpt-5.5", "account-2"),
		{ kind: "set_model", agent: "agent-1", model: "gpt-5.5", account: "account-2" },
	],
	["stopAgent", (s) => s.stopAgent("agent-1"), { kind: "stop_agent", agent: "agent-1" }],
	["deleteAgent", (s) => s.deleteAgent("agent-1"), { kind: "delete_agent", agent: "agent-1" }],
	[
		"moveAgent",
		(s) => s.moveAgent("device-1", "agent-1", "device-2", MOVE_TO, true),
		{
			kind: "move_agent",
			device: "device-1",
			agent: "agent-1",
			from: "device-2",
			to: MOVE_TO,
			stop: true,
		},
	],
	[
		"renameAgent",
		(s) => s.renameAgent("agent-1", "Release notes"),
		{ kind: "rename_agent", agent: "agent-1", title: "Release notes" },
	],
	["continueHere", (s) => s.continueHere("agent-1"), { kind: "continue_here", agent: "agent-1" }],
	[
		"takeOver",
		(s) => s.takeOver("agent-1", "after_turn"),
		{ kind: "take_over", agent: "agent-1", mode: "after_turn" },
	],
	[
		"answer",
		(s) => s.answer("attention-1", ANSWER),
		{ kind: "answer", event: "attention-1", answer: ANSWER },
	],
	[
		"answer with pills",
		(s) => s.answer("attention-1", { type: "text", text: "Like \uFFFC" }, [AGENT_PILL]),
		{
			kind: "answer",
			event: "attention-1",
			answer: { type: "text", text: "Like \uFFFC" },
			refs: [AGENT_PILL],
		},
	],
	[
		"setDeviceSettings",
		(s) => s.setDeviceSettings(SETTINGS),
		{ kind: "set_device_settings", settings: SETTINGS },
	],
	[
		"createWorkspace",
		(s) => s.createWorkspace(WORKSPACE),
		{ kind: "create_workspace", spec: WORKSPACE },
	],
	[
		"startWorkspace",
		(s) => s.startWorkspace("workspace-1"),
		{ kind: "start_workspace", workspace: "workspace-1" },
	],
	[
		"stopWorkspace",
		(s) => s.stopWorkspace("workspace-1"),
		{ kind: "stop_workspace", workspace: "workspace-1" },
	],
	[
		"deleteWorkspace",
		(s) => s.deleteWorkspace("workspace-1", true),
		{ kind: "delete_workspace", workspace: "workspace-1", discard_unpushed: true },
	],
	[
		"setProjectSettings",
		(s) => s.setProjectSettings(PROJECT),
		{ kind: "set_project_settings", settings: PROJECT },
	],
	[
		"setProjectHidden",
		(s) => s.setProjectHidden("project-1", true),
		{ kind: "set_project_hidden", project: "project-1", hidden: true },
	],
	[
		"removeCommandRule",
		(s) => s.removeCommandRule("device-1", "project-1", { command: "git show", exact: false }),
		{
			kind: "remove_command_rule",
			device: "device-1",
			project: "project-1",
			rule: { command: "git show", exact: false },
		},
	],
	[
		"removeAllowedEdit",
		(s) => s.removeAllowedEdit("device-1", "project-1", "git_hooks"),
		{
			kind: "remove_allowed_edit",
			device: "device-1",
			project: "project-1",
			edit: "git_hooks",
		},
	],
	[
		"windDown",
		(s) => s.windDown("device-1", 600),
		{ kind: "wind_down", device: "device-1", window_secs: 600 },
	],
	[
		"signInChatGpt",
		(s) => s.signInChatGpt("device-1"),
		{ kind: "sign_in_chatgpt", device: "device-1" },
	],
	[
		"checkGithubApp",
		(s) => s.checkGithubApp("device-1"),
		{ kind: "check_github_app", device: "device-1" },
	],
	[
		"forgetCredential",
		(s) => s.forgetCredential("device-1", "credential-1"),
		{ kind: "forget_credential", device: "device-1", credential: "credential-1" },
	],
	[
		"setAcpAgent",
		(s) => s.setAcpAgent("device-1", ACP_AGENT, [{ name: "API_KEY", value: "value-1" }]),
		{
			kind: "set_acp_agent",
			device: "device-1",
			agent: ACP_AGENT,
			env: [{ name: "API_KEY", value: "value-1" }],
		},
	],
	[
		"deleteAcpAgent",
		(s) => s.deleteAcpAgent("device-1", "acp-1"),
		{ kind: "delete_acp_agent", device: "device-1", id: "acp-1" },
	],
	[
		"modelAction",
		(s) =>
			s.modelAction("device-1", {
				action: "load",
				server: "srv-1",
				model: "qwen3.6-35b-a3b",
			}),
		{
			kind: "model_action",
			device: "device-1",
			action: { action: "load", server: "srv-1", model: "qwen3.6-35b-a3b" },
		},
	],
	[
		"grantPermission",
		(s) => s.grantPermission(PERMISSION),
		{ kind: "grant_permission", permission: PERMISSION },
	],
	[
		"revokePermission",
		(s) => s.revokePermission("permission-1"),
		{ kind: "revoke_permission", permission: "permission-1" },
	],
	[
		"mintKey",
		(s) => s.mintKey("device-1", "ci-bot", PERMISSION),
		{ kind: "mint_key", device: "device-1", name: "ci-bot", permission: PERMISSION },
	],
	[
		"saveTask",
		(s) => s.saveTask("device-1", TASK),
		{ kind: "save_task", device: "device-1", task: TASK },
	],
	[
		"setTaskEnabled",
		(s) => s.setTaskEnabled("device-1", "task-1", false),
		{ kind: "set_task_enabled", device: "device-1", task: "task-1", enabled: false },
	],
	[
		"askDispatcher",
		(s) => s.askDispatcher("device-1", "Review new PRs", "account-1", true),
		{
			kind: "ask_dispatcher",
			device: "device-1",
			text: "Review new PRs",
			account: "account-1",
			new: true,
		},
	],
	[
		"askDispatcher with pills",
		(s) => s.askDispatcher("device-1", "Ask \uFFFC", "account-1", false, [AGENT_PILL]),
		{
			kind: "ask_dispatcher",
			device: "device-1",
			text: "Ask \uFFFC",
			account: "account-1",
			new: false,
			refs: [AGENT_PILL],
		},
	],
	[
		"actOnChat",
		(s) => s.actOnChat("device-1", "agent-1", { type: "rename", title: "Slow login" }),
		{
			kind: "act_on_chat",
			device: "device-1",
			agent: "agent-1",
			action: { type: "rename", title: "Slow login" },
		},
	],
	[
		"readNote",
		(s) => s.readNote("device-1", "project-1", "note-1", null),
		{
			kind: "read_note",
			device: "device-1",
			project: "project-1",
			note: "note-1",
			version: null,
		},
	],
	[
		"saveNote",
		(s) =>
			s.saveNote("device-1", "project-1", "note-1", {
				base: "ab",
				title: "Release checklist",
				body: "- [ ] Tag\n",
				deleted: false,
			}),
		{
			kind: "save_note",
			device: "device-1",
			project: "project-1",
			note: "note-1",
			base: "ab",
			title: "Release checklist",
			body: "- [ ] Tag\n",
			deleted: false,
		},
	],
	[
		"noteHistory",
		(s) => s.noteHistory("device-1", "project-1", "note-1"),
		{ kind: "note_history", device: "device-1", project: "project-1", note: "note-1" },
	],
	[
		"confirmPlan",
		(s) => s.confirmPlan("device-1", 3),
		{ kind: "confirm_plan", device: "device-1", plan: 3 },
	],
	[
		"dismissPlan",
		(s) => s.dismissPlan("device-1", 3),
		{ kind: "dismiss_plan", device: "device-1", plan: 3 },
	],
	[
		"undoDispatcherAction",
		(s) => s.undoDispatcherAction("device-1", "key-1"),
		{ kind: "undo_dispatcher_action", device: "device-1", action: "key-1" },
	],
	["deleteTask", (s) => s.deleteTask("task-1"), { kind: "delete_task", task: "task-1" }],
	["runTaskNow", (s) => s.runTaskNow("task-1"), { kind: "run_task_now", task: "task-1" }],
	[
		"setHookOn",
		(s) => s.setHookOn("task-1", false),
		{ kind: "set_hook_on", task: "task-1", on: false },
	],
	[
		"setHookAddress",
		(s) => s.setHookAddress("device-1", { kind: "own", base: "https://hooks.example.com" }),
		{
			kind: "set_hook_address",
			device: "device-1",
			address: { kind: "own", base: "https://hooks.example.com" },
		},
	],
	[
		"rotateHookToken",
		(s) => s.rotateHookToken("task-1"),
		{ kind: "rotate_hook_token", task: "task-1" },
	],
	[
		"setHookSecret",
		(s) => s.setHookSecret("task-1", "test-not-a-real-secret"),
		{ kind: "set_hook_secret", task: "task-1", secret: "test-not-a-real-secret" },
	],
	[
		"listPulls",
		(s) => s.listPulls("project-1", "open"),
		{ kind: "list_pulls", project: "project-1", state: "open" },
	],
	[
		"listBranches",
		(s) => s.listBranches("project-1"),
		{ kind: "list_branches", project: "project-1" },
	],
	[
		"getPull",
		(s) => s.getPull("device-1", "owner/repo", 7),
		{ kind: "get_pull", device: "device-1", repo: "owner/repo", number: 7 },
	],
	[
		"pullFiles",
		(s) => s.pullFiles("device-1", "owner/repo", 7, 2),
		{ kind: "pull_files", device: "device-1", repo: "owner/repo", number: 7, page: 2 },
	],
	[
		"pullComments",
		(s) => s.pullComments("device-1", "owner/repo", 7),
		{ kind: "pull_comments", device: "device-1", repo: "owner/repo", number: 7 },
	],
	[
		"pullChecks",
		(s) => s.pullChecks("device-1", "owner/repo", 7),
		{ kind: "pull_checks", device: "device-1", repo: "owner/repo", number: 7 },
	],
	[
		"pullAction",
		(s) => s.pullAction("owner/repo", 7, { action: "merge", method: "squash" }, "abc123"),
		{
			kind: "pull_action",
			repo: "owner/repo",
			number: 7,
			action: { action: "merge", method: "squash" },
			expect_sha: "abc123",
		},
	],
	[
		"pullsDue",
		(s) => s.pullsDue("device-1", "owner/repo"),
		{ kind: "pulls_due", device: "device-1", repo: "owner/repo" },
	],
	[
		"linkPull",
		(s) => s.linkPull("agent-1", 7),
		{ kind: "link_pull", agent: "agent-1", number: 7 },
	],
	[
		"unlinkPull",
		(s) => s.unlinkPull("agent-1", "owner/repo", 7),
		{ kind: "unlink_pull", agent: "agent-1", repo: "owner/repo", number: 7 },
	],
	[
		"beginGithubSignIn",
		(s) => s.beginGithubSignIn("device-1", null),
		{ kind: "begin_github_sign_in", device: "device-1", client_id: null },
	],
	[
		"cancelGithubSignIn",
		(s) => s.cancelGithubSignIn("device-1", "sign-in-1"),
		{ kind: "cancel_github_sign_in", device: "device-1", sign_in: "sign-in-1" },
	],
	[
		"checkGithubSignIn",
		(s) => s.checkGithubSignIn("device-1", "sign-in-1"),
		{ kind: "check_github_sign_in", device: "device-1", sign_in: "sign-in-1" },
	],
	[
		"beginCliLogin",
		(s) => s.beginCliLogin("device-1", "account-1", true),
		{ kind: "begin_cli_login", device: "device-1", account: "account-1", device_code: true },
	],
	[
		"finishCliLogin",
		(s) => s.finishCliLogin("device-1", "sign-in-1", "code-1"),
		{ kind: "finish_cli_login", device: "device-1", sign_in: "sign-in-1", code: "code-1" },
	],
	[
		"checkCliLogin",
		(s) => s.checkCliLogin("device-1", "sign-in-1"),
		{ kind: "check_cli_login", device: "device-1", sign_in: "sign-in-1" },
	],
	[
		"cancelCliLogin",
		(s) => s.cancelCliLogin("device-1", "sign-in-1"),
		{ kind: "cancel_cli_login", device: "device-1", sign_in: "sign-in-1" },
	],
	[
		"addSkillSource",
		(s) => s.addSkillSource("device-1", "agent-skills", ORIGIN, null, "own"),
		{
			kind: "skill_source_add",
			device: "device-1",
			name: "agent-skills",
			origin: ORIGIN,
			default_ref: null,
			trust: "own",
		},
	],
	[
		"fetchSkillSource",
		(s) => s.fetchSkillSource("device-1", "source-1"),
		{ kind: "skill_source_fetch", device: "device-1", source: "source-1" },
	],
	[
		"removeSkillSource",
		(s) => s.removeSkillSource("device-1", "source-1"),
		{ kind: "skill_source_remove", device: "device-1", source: "source-1" },
	],
	[
		"setProjectSkills",
		(s) => s.setProjectSkills("device-1", "project-1", LOCK),
		{ kind: "skills_set_project", device: "device-1", project: "project-1", lock: LOCK },
	],
	[
		"applySkills",
		(s) => s.applySkills("device-1", "project-1"),
		{ kind: "skills_apply", device: "device-1", project: "project-1" },
	],
	[
		"scanSkills",
		(s) => s.scanSkills("device-1", SPEC.place),
		{ kind: "skills_scan", device: "device-1", place: SPEC.place },
	],
	[
		"skillTreesNeeded",
		(s) => s.skillTreesNeeded("device-1", LOCK),
		{ kind: "skills_trees_needed", device: "device-1", lock: LOCK },
	],
	[
		"answerSkillProposal",
		(s) => s.answerSkillProposal("proposal-1", "approve", true),
		{
			kind: "skill_proposal_action",
			proposal: "proposal-1",
			action: "approve",
			push: true,
			fork: null,
			seen: null,
		},
	],
	[
		"answerSkillProposal",
		(s) => s.answerSkillProposal("proposal-1", "approve", false, "my-skill"),
		{
			kind: "skill_proposal_action",
			proposal: "proposal-1",
			action: "approve",
			push: false,
			fork: "my-skill",
			seen: null,
		},
	],
	[
		"answerSkillProposal",
		(s) => s.answerSkillProposal("proposal-1", "approve", false, undefined, SEEN),
		{
			kind: "skill_proposal_action",
			proposal: "proposal-1",
			action: "approve",
			push: false,
			fork: null,
			seen: SEEN,
		},
	],
	[
		"skillProposalDiff",
		(s) => s.skillProposalDiff("proposal-1"),
		{ kind: "skill_proposal_diff", proposal: "proposal-1" },
	],
	[
		"planGlobalSkills",
		(s) => s.planGlobalSkills("device-1"),
		{ kind: "skills_globals_plan", device: "device-1" },
	],
	[
		"applyGlobalSkills",
		(s) => s.applyGlobalSkills("device-1", "plan-1", ["/home/me/.claude/skills/review"]),
		{
			kind: "skills_globals_apply",
			device: "device-1",
			plan_hash: "plan-1",
			picks: ["/home/me/.claude/skills/review"],
		},
	],
	[
		"undoGlobalSkills",
		(s) => s.undoGlobalSkills("device-1"),
		{ kind: "skills_globals_undo", device: "device-1" },
	],
	[
		"moveSkillLibrary",
		(s) => s.moveSkillLibrary("device-1"),
		{ kind: "move_skill_library", device: "device-1" },
	],
	[
		"pushSkillSource",
		(s) => s.pushSkillSource("device-1", "source-1"),
		{ kind: "skill_source_push", device: "device-1", source: "source-1" },
	],
	["checkHost", (s) => s.checkHost("device-1"), { kind: "check_host", device: "device-1" }],
	[
		"applyHost",
		(s) => s.applyHost("device-1", ["caps.parent"]),
		{ kind: "apply_host", device: "device-1", items: ["caps.parent"] },
	],
	[
		"installTool",
		(s) => s.installTool("device-1", HOST, NODE),
		{ kind: "install_tool", device: "device-1", place: HOST, tool: NODE, version: null },
	],
	[
		"removeTool",
		(s) => s.removeTool("device-1", HOST, NODE),
		{ kind: "remove_tool", device: "device-1", place: HOST, tool: NODE },
	],
	[
		"listSnapshots",
		(s) => s.listSnapshots("workspace-1"),
		{ kind: "list_snapshots", workspace: "workspace-1" },
	],
	[
		"restoreSnapshot",
		(s) => s.restoreSnapshot("workspace-1", "snap-1", false, true),
		{
			kind: "restore_snapshot",
			workspace: "workspace-1",
			snapshot: "snap-1",
			as_new: false,
			discard_unsaved: true,
		},
	],
	[
		"planMove",
		(s) => s.planMove("device-1", "ws-alpha"),
		{ kind: "plan_move", device: "device-1", instance: "ws-alpha" },
	],
	[
		"runMoveStep",
		(s) => s.runMoveStep("move-1", "copy"),
		{ kind: "run_move_step", moving: "move-1", step: "copy" },
	],
	["moveBack", (s) => s.moveBack("move-1"), { kind: "move_back", moving: "move-1" }],
	["retireMove", (s) => s.retireMove("move-1"), { kind: "retire_move", moving: "move-1" }],
	[
		"installAcpAgent",
		(s) => s.installAcpAgent("device-1", "gemini", "0.12.0", HOST, true),
		{
			kind: "install_acp_agent",
			device: "device-1",
			agent: "gemini",
			version: "0.12.0",
			place: HOST,
			confirm_unchecked: true,
		},
	],
	[
		"acceptLicence",
		(s) => s.acceptLicence("device-1", "android-sdk-license", "abc123"),
		{
			kind: "accept_licence",
			device: "device-1",
			licence: "android-sdk-license",
			text_sha256: "abc123",
		},
	],
	[
		"openTerminal",
		(s) => s.openTerminal("agent-1", { type: "shell" }, { cols: 120, rows: 32 }),
		{ kind: "terminal_open", agent: "agent-1", runs: { type: "shell" }, cols: 120, rows: 32 },
	],
	[
		"openTerminal for a command",
		(s) =>
			s.openTerminal(
				"agent-1",
				{ type: "run", command: "pnpm test", private: true },
				{ cols: 80, rows: 24 }
			),
		{
			kind: "terminal_open",
			agent: "agent-1",
			runs: { type: "run", command: "pnpm test", private: true },
			cols: 80,
			rows: 24,
		},
	],
	[
		"interruptTerminal",
		(s) => s.interruptTerminal("terminal-1"),
		{ kind: "terminal_interrupt", terminal: "terminal-1" },
	],
	[
		"interruptTerminal by force",
		(s) => s.interruptTerminal("terminal-1", true),
		{ kind: "terminal_interrupt", terminal: "terminal-1", force: true },
	],
	[
		"closeTerminal",
		(s) => s.closeTerminal("terminal-1"),
		{ kind: "terminal_close", terminal: "terminal-1", force: false },
	],
	[
		"closeTerminal by force",
		(s) => s.closeTerminal("terminal-1", true),
		{ kind: "terminal_close", terminal: "terminal-1", force: true },
	],
	[
		"renameTerminal",
		(s) => s.renameTerminal("terminal-1", "dev server"),
		{ kind: "terminal_rename", terminal: "terminal-1", title: "dev server" },
	],
	[
		"moveTerminal",
		(s) => s.moveTerminal("terminal-1"),
		{ kind: "terminal_move", terminal: "terminal-1" },
	],
];

/** The requests that can fail but answer with nothing. */
export const UNITS: Case[] = [
	["markSeen", (s) => s.markSeen("agent-1"), { kind: "mark_seen", agent: "agent-1" }],
	["refreshUsage", (s) => s.refreshUsage(null), { kind: "refresh_usage", account: null }],
	["refreshLinks", (s) => s.refreshLinks("link-1"), { kind: "refresh_links", link: "link-1" }],
	["restartMaxd", (s) => s.restartMaxd("link-1"), { kind: "restart_maxd", link: "link-1" }],
	["removeDevice", (s) => s.removeDevice("link-1"), { kind: "remove_device", link: "link-1" }],
	["startLocalMaxd", (s) => s.startLocalMaxd(), { kind: "start_local_maxd" }],
	[
		"uninstallLocalMaxd",
		(s) => s.uninstallLocalMaxd(true),
		{ kind: "uninstall_local_maxd", delete_data: true },
	],
	[
		"loadOlder",
		(s) => s.loadOlder("agent-1", 40),
		{ kind: "load_older", agent: "agent-1", before: 40 },
	],
	[
		"setAgentDefaults",
		(s) => s.setAgentDefaults({ limits: NO_LIMITS, depth: 2 }),
		{ kind: "set_agent_defaults", defaults: { limits: NO_LIMITS, depth: 2 } },
	],
	[
		"setDeviceLink",
		(s) => s.setDeviceLink("device-1", "device-2", false),
		{ kind: "set_device_link", device: "device-1", peer: "device-2", on: false },
	],
	[
		"attachTerminal",
		(s) => s.attachTerminal("terminal-1", 4096),
		{ kind: "terminal_attach", terminal: "terminal-1", after: 4096 },
	],
	[
		"attachTerminal from the start",
		(s) => s.attachTerminal("terminal-1", null),
		{ kind: "terminal_attach", terminal: "terminal-1", after: null },
	],
	[
		"detachTerminal",
		(s) => s.detachTerminal("terminal-1"),
		{ kind: "terminal_detach", terminal: "terminal-1" },
	],
	[
		"resizeTerminal",
		(s) => s.resizeTerminal("terminal-1", { cols: 100, rows: 30 }),
		{ kind: "terminal_resize", terminal: "terminal-1", cols: 100, rows: 30 },
	],
];
