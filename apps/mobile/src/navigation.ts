// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { NativeStackScreenProps } from "@react-navigation/native-stack";

import type { AgentId, AttentionId, NoteId, ProjectId } from "@maxhq/shared/bindings";

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
};

/**
 * The Agent screen's ID, so each agent gets its own entry in the stack.
 * Without one, opening another agent from a push reuses the screen on top
 * and only swaps its params, and its draft goes with it.
 */
export const agentScreenId = ({ params }: { readonly params: Screens["Agent"] }): string =>
	params.agent;

/** The Note screen's ID, so each note gets its own entry in the stack, with its own saving. */
export const noteScreenId = ({ params }: { readonly params: Screens["Note"] }): string =>
	`${params.project}/${params.note}`;

export type ScreenProps<Name extends keyof Screens> = NativeStackScreenProps<Screens, Name>;
