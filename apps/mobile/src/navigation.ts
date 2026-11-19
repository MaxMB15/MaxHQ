// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { NativeStackScreenProps } from "@react-navigation/native-stack";

import type {
	AgentId,
	AttentionId,
	DeviceId,
	LeaseId,
	LinkId,
	NoteId,
	ProjectId,
	SkillProposalId,
	TaskId,
	WorkspaceId,
} from "@maxhq/shared/bindings";
import { runName } from "@maxhq/shared/workflow/select";

/** The screens and what each takes. */
export type Screens = {
	Inbox: undefined;
	Agent: { readonly agent: AgentId; readonly attention?: AttentionId };
	Usage: undefined;
	Devices: undefined;
	/** `byHand` opens the form to add a device by hand. */
	Settings: { readonly byHand?: boolean } | undefined;
	/** `code` fills the paste box, as from a code opened in the Camera app. */
	Scan: { readonly code?: string } | undefined;
	/** A project's notes. */
	Notes: { readonly project: ProjectId };
	/** One note. `made` opens a note just made with its title ready to type. */
	Note: { readonly project: ProjectId; readonly note: NoteId; readonly made?: boolean };
	/** A file on the agent's device, read-only, scrolled to `line` when it's given. */
	File: { readonly agent: AgentId; readonly path: string; readonly line?: number };
	/** A device's page, by the link the phone reaches it through. */
	Device: { readonly link: LinkId };
	/** The licences a device waits on, to read and accept. */
	Licences: { readonly device: DeviceId };
	/** The form that makes a workspace on a device. */
	NewWorkspace: { readonly device: DeviceId };
	/** A test machine an agent leases: what it is, its screen, and End lease. */
	Lease: { readonly device: DeviceId; readonly lease: LeaseId };
	/** A project's pull requests, or the first project with any when none is given. */
	Pulls: { readonly project?: ProjectId } | undefined;
	/** One pull request, read on `device`. */
	Pull: { readonly device: DeviceId; readonly repo: string; readonly number: number };
	/** Every device's scheduled tasks. */
	Tasks: undefined;
	/** A task's runs and the requests its webhook took. */
	TaskActivity: { readonly task: TaskId };
	/** A task to edit, or a new one on `home`, or on the device that suits it best. */
	TaskEditor: { readonly task?: TaskId; readonly home?: DeviceId } | undefined;
	/** A page Settings opens, as the desktop's App group has them. */
	SettingsPage: {
		readonly page: "agent-list" | "composer" | "notifications" | "dispatcher" | "agent-tools";
	};
	/** A login's page, with `device` first when it's given. */
	SignIn: {
		readonly login: "claude" | "codex" | "cursor" | "github" | "apns";
		readonly device?: DeviceId;
	};
	/** One device's settings. */
	DeviceSettings: { readonly device: DeviceId };
	/** A project's settings on each device it's on. */
	ProjectSettings: { readonly project: ProjectId };
	/** An agent's details: its context window, branch, who started it, and what it may run. */
	AgentInfo: { readonly agent: AgentId };
	/** The form that moves an agent to another device. */
	MoveAgent: { readonly agent: AgentId };
	/** The ports an agent's workspace listens on and the processes it left running. */
	Running: { readonly agent: AgentId };
	/** A lead agent's sub-agents and its context window. */
	SubAgents: { readonly agent: AgentId };
	/** The form that starts an agent, in `project` when one is given. */
	NewAgent: { readonly project?: ProjectId } | undefined;
	/** A project's skills on each device that keeps them, with its proposals. */
	Skills: { readonly project: ProjectId };
	/** The skill library: its sources, the skills every agent gets, and each device's global skills. */
	SkillLibrary: undefined;
	/** One skill proposal with its diff, to approve or reject. */
	SkillProposal: { readonly proposal: SkillProposalId };
	/** A project's workflow runs, live ones first. */
	Workflows: { readonly project: ProjectId };
	/** One workflow run, by its key: its steps and its timeline. */
	Run: {
		readonly device: DeviceId;
		readonly project: ProjectId;
		readonly workflow: string;
		readonly run: string;
	};
	/** The form that copies a project to another device, for a project, a checkout or a device. */
	CopyProject:
		| {
				readonly project?: ProjectId;
				readonly from?: WorkspaceId;
				readonly device?: DeviceId;
		  }
		| undefined;
};

/**
 * The Agent screen's ID, so each agent gets its own entry in the stack.
 * Without one, opening another agent from a push reuses the screen on top
 * and only swaps its params, and its draft goes with it.
 */
export const agentScreenId = ({ params }: { readonly params: Screens["Agent"] }): string =>
	params.agent;

/** The ID of a screen about one agent, so each agent gets its own. */
export const agentPageId = ({ params }: { readonly params: { readonly agent: AgentId } }): string =>
	params.agent;

/** The Note screen's ID, so each note gets its own entry in the stack, with its own saving. */
export const noteScreenId = ({ params }: { readonly params: Screens["Note"] }): string =>
	`${params.project}/${params.note}`;

/**
 * The File screen's ID: one screen for each of an agent's files, so asking
 * for another line of a file that's open scrolls the screen already there.
 */
export const fileScreenId = ({ params }: { readonly params: Screens["File"] }): string =>
	`${params.agent}\n${params.path}`;

/** The Device screen's ID: one screen for each device. */
export const deviceScreenId = ({ params }: { readonly params: Screens["Device"] }): string =>
	params.link;

/** The Lease screen's ID: one screen for each lease. */
export const leaseScreenId = ({ params }: { readonly params: Screens["Lease"] }): string =>
	`${params.device}\n${params.lease}`;

/**
 * The Pull screen's ID, so each PR gets its own entry in the stack. A
 * repository is the same in any case.
 */
export const pullScreenId = ({ params }: { readonly params: Screens["Pull"] }): string =>
	`${params.device}\n${params.repo.toLowerCase()}#${params.number}`;

/** The Run screen's ID: one screen for each run. */
export const runScreenId = ({ params }: { readonly params: Screens["Run"] }): string =>
	runName(params);

export type ScreenProps<Name extends keyof Screens> = NativeStackScreenProps<Screens, Name>;
