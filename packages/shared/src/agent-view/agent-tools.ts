// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a call to one of MaxHQ's agent tools reads in the timeline: "Started
// Fix the login bug on devbox" rather than mcp__maxhq__start_agent and its
// JSON. maxd splits Claude's mcp__maxhq__<tool> into the server and the
// tool, and Codex sends the two apart, so a call is one of them when its
// server is maxhq.

import { t } from "../i18n";

import { plural } from "../status/format";
import { deviceName } from "../view/select";
import type { ViewState } from "../view/replica";

/** The agent tools, by the name the daemon gives them. */
export type AgentTool =
	| "list_targets"
	| "list_agents"
	| "read_agent"
	| "start_agent"
	| "send_agent"
	| "wait_agent"
	| "interrupt_agent"
	| "move_agent";

const TOOLS = new Set<string>([
	"list_targets",
	"list_agents",
	"read_agent",
	"start_agent",
	"send_agent",
	"wait_agent",
	"interrupt_agent",
	"move_agent",
]);

const SERVER = "maxhq";

/** Which agent tool a tool call is, if it's one. */
export const agentToolOf = (server: string | null, name: string): AgentTool | undefined =>
	server === SERVER && TOOLS.has(name) ? (name as AgentTool) : undefined;

/** The names a row shows for agents and devices. */
export type ToolNames = {
	/** The agent's title, if the view has it. */
	readonly agent: (id: string) => string | undefined;
	/** The device's name. */
	readonly device: (id: string) => string;
};

/** Names from the view. */
export const viewNames = (view: ViewState): ToolNames => ({
	agent: (id) => view.tables.agent.get(id)?.title || undefined,
	device: (id) => deviceName(view, id),
});

/** What an agent tool row says. */
export type AgentToolLine = {
	readonly tool: AgentTool;
	/** The verb while it runs. */
	readonly doing: string;
	/** The verb once it ended. */
	readonly done: string;
	/** What it acted on, after the verb. */
	readonly target: string;
	/** A word for the end of the row, such as when a wait ran out. */
	readonly note: string | null;
};

type Json = Record<string, unknown>;

const parse = (text: string | null): Json => {
	if (!text) return {};
	try {
		const value: unknown = JSON.parse(text);
		return typeof value === "object" && value !== null && !Array.isArray(value)
			? (value as Json)
			: {};
	} catch {
		return {};
	}
};

const str = (value: unknown): string | undefined =>
	typeof value === "string" && value !== "" ? value : undefined;

const list = (value: unknown): unknown[] | undefined => (Array.isArray(value) ? value : undefined);

/** The call's input and output, as the timeline item carries them. */
export type ToolCallText = {
	readonly server: string | null;
	readonly name: string;
	readonly input: string | null;
	readonly output: string | null;
};

/**
 * The row for an agent tool call, or undefined for any other tool. It
 * reads what it can from the input and output, and names agents and
 * devices through `names`.
 */
export const agentToolLine = (call: ToolCallText, names: ToolNames): AgentToolLine | undefined => {
	const tool = agentToolOf(call.server, call.name);
	if (tool === undefined) return undefined;
	const input = parse(call.input);
	const output = parse(call.output);

	const agentName = (value: unknown, fallback = t("an agent")) => {
		const id = str(value);
		return (id && names.agent(id)) ?? fallback;
	};
	const onDevice = (value: unknown) => {
		const id = str(value);
		return id ? t(" on {{value1}}", { value1: names.device(id) }) : "";
	};
	const line = (doing: string, done: string, target: string, note: string | null = null) => ({
		tool,
		doing,
		done,
		target,
		note,
	});

	switch (tool) {
		case "list_targets":
			return line(t("Checking"), t("Checked"), t("where agents can start"));
		case "list_agents": {
			const found = list(output.agents);
			return line(
				t("Listing"),
				t("Listed"),
				found ? plural(found.length, "agent") : "agents"
			);
		}
		case "read_agent":
			return line(t("Reading"), t("Read"), agentName(input.agent));
		case "start_agent": {
			const named = str(input.title) ?? t("an agent");
			const started = agentName(output.agent, named);
			return line(
				t("Starting"),
				t("Started"),
				`${started}${onDevice(output.device ?? input.device)}`
			);
		}
		case "send_agent":
			return line(t("Messaging"), t("Messaged"), agentName(input.agent));
		case "wait_agent": {
			const waited = list(input.agents) ?? [];
			const target =
				waited.length === 1 ? agentName(waited[0]) : plural(waited.length, "agent");
			return line(
				t("Waiting on"),
				t("Waited on"),
				target,
				output.timed_out === true ? t("Timed out") : null
			);
		}
		case "interrupt_agent":
			return input.stop === true
				? line(t("Stopping"), t("Stopped"), agentName(input.agent))
				: line(t("Interrupting"), t("Interrupted"), agentName(input.agent));
		case "move_agent": {
			const to = str(output.device) ?? str(input.to_device);
			const there = to ? t(" to {{value1}}", { value1: names.device(to) }) : "";
			return line(t("Moving"), t("Moved"), `${agentName(input.agent)}${there}`);
		}
	}
};
