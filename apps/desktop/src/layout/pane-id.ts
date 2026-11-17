// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pane's ID and what it's given, in one place. A pane shows an agent, a
// workflow run, a project note, a file an agent can see, a visual or a
// terminal you took out of an agent's drawer. An agent's pane
// takes the agent's ID, as it always has, so saved layouts still load, and
// the others take their key behind a prefix no agent ID has. Code that reads a pane's
// ID goes through these calls, so the kinds never mix.

import type { AgentId, NoteId, ProjectId, RunKey, TerminalId } from "@maxhq/shared/bindings";

/** What a pane is given. */
export type PaneParams =
	| { readonly kind: "agent"; readonly agent: AgentId }
	| { readonly kind: "workflow"; readonly run: RunKey }
	| { readonly kind: "note"; readonly project: ProjectId; readonly note: NoteId }
	| { readonly kind: "file"; readonly agent: AgentId; readonly path: string }
	/** A visual by its `visualKey`. Its page stays in memory, out of the saved layout. */
	| { readonly kind: "visual"; readonly key: string; readonly title: string }
	/** A terminal out of its agent's drawer, which goes back there when the pane closes. */
	| { readonly kind: "terminal"; readonly agent: AgentId; readonly terminal: TerminalId };

const RUN = "workflow:";
const NOTE = "note:";
const FILE = "file:";
const VISUAL = "visual:";
const TERMINAL = "terminal:";

const joined = (parts: readonly string[]) => parts.map(encodeURIComponent).join("/");
const split = (rest: string) => rest.split("/").map(decodeURIComponent);

/** The ID of the pane with these params. */
export const paneId = (params: PaneParams): string => {
	if (params.kind === "agent") return params.agent;
	if (params.kind === "note") return NOTE + joined([params.project, params.note]);
	if (params.kind === "file") return FILE + joined([params.agent, params.path]);
	if (params.kind === "visual") return VISUAL + joined([params.key, params.title]);
	if (params.kind === "terminal") return TERMINAL + joined([params.agent, params.terminal]);
	const { device, project, workflow, run } = params.run;
	return RUN + joined([device, project, workflow, run]);
};

/** What the pane with this ID is given. */
export const paneParams = (id: string): PaneParams => {
	if (id.startsWith(NOTE)) {
		const [project = "", note = ""] = split(id.slice(NOTE.length));
		return { kind: "note", project, note };
	}
	if (id.startsWith(FILE)) {
		const [agent = "", path = ""] = split(id.slice(FILE.length));
		return { kind: "file", agent, path };
	}
	if (id.startsWith(VISUAL)) {
		const [key = "", title = ""] = split(id.slice(VISUAL.length));
		return { kind: "visual", key, title };
	}
	if (id.startsWith(TERMINAL)) {
		const [agent = "", terminal = ""] = split(id.slice(TERMINAL.length));
		return { kind: "terminal", agent, terminal };
	}
	if (!id.startsWith(RUN)) return { kind: "agent", agent: id };
	const [device = "", project = "", workflow = "", run = ""] = split(id.slice(RUN.length));
	return { kind: "workflow", run: { device, project, workflow, run } };
};

/** The agent the pane with this ID shows, or null for a pane of another kind. */
export const paneAgent = (id: string): AgentId | null => {
	const params = paneParams(id);
	return params.kind === "agent" ? params.agent : null;
};
