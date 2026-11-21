// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The workspace, copy, editor, login, transfer, forward, process, lookup,
// fleet, agent tools, scheduled task, dispatcher, PR and terminal calls
// for a hand-made source in a test that doesn't use them. Each one says
// it's done, git knows no identity, this Mac has no editors, no workspace
// reaches one over SSH, the lookups find nothing, no agent runs a process,
// no device holds a permission or a key, no device has a task or PR, and
// no maxd has terminals.

import {
	AGENT_DEFAULTS,
	type ActionResult,
	type CommandResult,
	type DataSource,
} from "../data/source";
import { noCliLogins } from "../data/mock-cli-login";
import { noHosts } from "../data/mock-host-calls";
import { noLocalModels } from "../data/mock-local-models";
import { noPulls } from "../data/mock-pulls";
import { noSkills } from "../data/mock-skills";
import { noTasks } from "../data/mock-tasks";
import { noTerminals } from "../data/mock-terminals";
import { noFolders } from "../data/mock-folders";
import { noNotices } from "../data/mock-notices";
import { noSessions } from "../data/mock-sessions";
import { noTestHosts } from "../data/mock-test-hosts";

type RemoteCalls = Pick<
	DataSource,
	| "createWorkspace"
	| "startWorkspace"
	| "stopWorkspace"
	| "deleteWorkspace"
	| "deleteAgent"
	| "moveAgent"
	| "renameAgent"
	| "listIgnoredFiles"
	| "copyProject"
	| "cancelClone"
	| "retryClone"
	| "removeClone"
	| "sshInclude"
	| "setupSshInclude"
	| "listEditors"
	| "openInEditor"
	| "setProjectSettings"
	| "setProjectHidden"
	| "removeCommandRule"
	| "gitIdentity"
	| "windDown"
	| "restartMaxd"
	| "removeDevice"
	| "startLocalMaxd"
	| "uninstallLocalMaxd"
	| "storeClaudeToken"
	| "storeCursorKey"
	| "signInChatGpt"
	| "setGithubApp"
	| "checkGithubApp"
	| "setApnsKey"
	| "forgetCredential"
	| "setAcpAgent"
	| "deleteAcpAgent"
	| "upload"
	| "uploadPasted"
	| "attachFromAgent"
	| "cancelTransfer"
	| "download"
	| "media"
	| "openForward"
	| "closeForward"
	| "openLocalhost"
	| "listProcesses"
	| "signalProcess"
	| "readNote"
	| "saveNote"
	| "noteHistory"
	| "findPaths"
	| "stageFound"
	| "findGitItems"
	| "excerptAgent"
	| "findInTimeline"
	| "readLoad"
	| "readDiagnostics"
	| "updateTools"
	| "updateFleet"
	| "setupDevice"
	| "confirmHostKey"
	| "setRoutes"
	| "listSetupHosts"
	| "listPermissions"
	| "grantPermission"
	| "revokePermission"
	| "mintKey"
	| "listKeys"
	| "listDeviceLinks"
	| "setDeviceLink"
	| "listPhoneDevices"
	| "beginPhonePair"
	| "removePhone"
	| "setPhoneEverything"
	| "renewPhoneCodes"
	| "setPhoneAddress"
	| "revokePhoneSession"
	| "appKey"
	| "removeAppKey"
	| "rotateAppKey"
	| "answerAdmission"
	| "agentDefaults"
	| "setAgentDefaults"
	| "askDispatcher"
	| "confirmPlan"
	| "dismissPlan"
	| "undoDispatcherAction"
	| "actOnChat"
	| "answerLinkClaim"
	| "askLinkClaimAgain"
	| "forgetLinkIdentity"
	| "linkClaimDetails"
	| keyof typeof noTasks
	| keyof typeof noPulls
	| keyof typeof noTestHosts
	| keyof typeof noSkills
	| keyof typeof noHosts
	| keyof typeof noLocalModels
	| keyof typeof noCliLogins
	| keyof typeof noTerminals
	| keyof typeof noFolders
	| keyof typeof noNotices
	| keyof typeof noSessions
>;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const OK: CommandResult = { status: "ok", data: null };

const done = async () => DONE;

const each = async (list: readonly unknown[]) => list.map(() => DONE);

/** Calls that answer done, with nothing behind them. */
export const remoteCalls = (): RemoteCalls => ({
	createWorkspace: done,
	startWorkspace: done,
	stopWorkspace: done,
	deleteWorkspace: done,
	deleteAgent: done,
	moveAgent: done,
	renameAgent: done,
	listIgnoredFiles: async () => ({ files: [], truncated: false }),
	copyProject: async () => ({ workspace: "workspace-copy", written: 0, failed: [] }),
	cancelClone: done,
	retryClone: done,
	removeClone: done,
	sshInclude: async () => ({ type: "unavailable", reason: "Not a container" }),
	setupSshInclude: async () => ({ type: "unavailable", reason: "Not a container" }),
	listEditors: async () => ({ available: [], known: [] }),
	openInEditor: async () => ({ status: "ok", data: null }),
	setProjectSettings: done,
	setProjectHidden: done,
	removeCommandRule: done,
	gitIdentity: async () => ({ name: null, email: null }),
	windDown: done,
	restartMaxd: async () => OK,
	removeDevice: async () => OK,
	startLocalMaxd: async () => OK,
	uninstallLocalMaxd: async () => OK,
	storeClaudeToken: each,
	storeCursorKey: each,
	signInChatGpt: done,
	setGithubApp: each,
	checkGithubApp: done,
	setApnsKey: each,
	forgetCredential: done,
	setAcpAgent: done,
	deleteAcpAgent: done,
	upload: (_device, _target, paths) => each(paths),
	uploadPasted: done,
	attachFromAgent: done,
	cancelTransfer: async () => {},
	download: async () => ({ status: "ok", data: { path: "/Users/someone/Downloads/MaxHQ/a" } }),
	media: async () => ({ status: "ok", data: { kind: "too_large", size: 0 } }),
	openForward: async () => ({ status: "ok", data: { forward: "forward-1", local_port: 8000 } }),
	closeForward: async () => {},
	openLocalhost: async () => {},
	listProcesses: async () => ({ status: "ok", data: { status: "listed", processes: [] } }),
	signalProcess: done,
	readNote: async () => ({ status: "error", error: { kind: "failed", message: "No notes" } }),
	saveNote: async () => ({ status: "error", error: { kind: "failed", message: "No notes" } }),
	noteHistory: async () => ({ status: "ok", data: [] }),
	findPaths: async () => [],
	stageFound: async (_agent, paths) => each(paths),
	findGitItems: async () => ({ repo: null, items: [], unavailable: null }),
	excerptAgent: async () => ({ name: "agent.md", text: "", messages: 0, omitted: 0 }),
	findInTimeline: async () => ({ total: 0, active: 0, found: null, near: [] }),
	readLoad: async () => [],
	readDiagnostics: async () => ({
		status: "ok",
		data: { buckets: [], top: [], containers: [], error: null },
	}),
	updateTools: done,
	updateFleet: async () => OK,
	setupDevice: async () => ({ status: "ok", data: "link-new" }),
	confirmHostKey: async () => OK,
	setRoutes: async () => OK,
	listSetupHosts: async () => [],
	listPermissions: async () => ({ status: "ok", data: [] }),
	grantPermission: done,
	revokePermission: done,
	mintKey: async (_device, _name, permission) => ({
		status: "ok",
		data: {
			status: "key_minted",
			id: "key-1",
			path: "/home/someone/.local/share/maxhq/keys/a.key",
			permission: permission.id,
			command: {
				command: "/home/someone/.local/bin/maxd",
				args: ["mcp", "--key-file", "/home/someone/.local/share/maxhq/keys/a.key"],
			},
		},
	}),
	listKeys: async () => ({ status: "ok", data: [] }),
	listDeviceLinks: async () => ({ status: "ok", data: [] }),
	setDeviceLink: async () => OK,
	listPhoneDevices: async () => ({ status: "ok", data: [] }),
	beginPhonePair: async () => ({
		status: "error",
		error: { kind: "failed", message: "No device takes phones" },
	}),
	removePhone: async () => OK,
	setPhoneEverything: async () => ({ status: "ok", data: [] }),
	renewPhoneCodes: async () => ({ status: "ok", data: [] }),
	setPhoneAddress: async () => OK,
	revokePhoneSession: done,
	appKey: async () => ({
		status: "error",
		error: { kind: "failed", message: "This Mac has no app key" },
	}),
	removeAppKey: done,
	rotateAppKey: async () => OK,
	answerAdmission: done,
	agentDefaults: async () => AGENT_DEFAULTS,
	setAgentDefaults: async () => OK,
	askDispatcher: done,
	confirmPlan: done,
	dismissPlan: done,
	undoDispatcherAction: done,
	actOnChat: done,
	answerLinkClaim: async () => OK,
	askLinkClaimAgain: async () => OK,
	forgetLinkIdentity: async () => OK,
	linkClaimDetails: async () => ({
		status: "error",
		error: { kind: "failed", message: "No link waits for an answer" },
	}),
	...noTasks,
	...noPulls,
	...noTestHosts,
	...noSkills,
	...noHosts,
	...noLocalModels,
	...noCliLogins,
	...noTerminals,
	...noFolders,
	...noNotices,
	...noSessions,
});
