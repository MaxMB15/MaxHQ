// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agentToolLine, agentToolOf, type ToolNames } from "./agent-tools";

const names: ToolNames = {
	agent: (id) => ({ "agent-a": "Fix the login bug", "agent-b": "Write the notes" })[id],
	device: (id) => ({ "dev-mac": "Mac", "dev-devbox": "devbox" })[id] ?? "Unknown device",
};

const call = (name: string, input: object | null, output: object | null = null) => ({
	server: "maxhq",
	name,
	input: input && JSON.stringify(input),
	output: output && JSON.stringify(output),
});

const said = (line: ReturnType<typeof agentToolLine>) =>
	line && `${line.done} ${line.target}${line.note ? ` (${line.note})` : ""}`;

describe("agentToolOf", () => {
	it("knows the tools by the maxhq server", () => {
		expect(agentToolOf("maxhq", "start_agent")).toBe("start_agent");
		expect(agentToolOf("maxhq", "wait_agent")).toBe("wait_agent");
	});

	it("leaves other tools alone", () => {
		expect(agentToolOf("github", "start_agent")).toBeUndefined();
		expect(agentToolOf("maxhq", "send_attachments")).toBeUndefined();
		expect(agentToolOf(null, "Bash")).toBeUndefined();
		// maxd splits the server out, so a whole name on its own isn't one.
		expect(agentToolOf(null, "mcp__maxhq__wait_agent")).toBeUndefined();
	});
});

describe("agentToolLine", () => {
	it("names the agent a call started and where", () => {
		const line = agentToolLine(
			call(
				"start_agent",
				{ task: "x", title: "Fix it", device: "dev-devbox" },
				{
					agent: "agent-a",
					device: "dev-devbox",
				}
			),
			names
		);
		expect(line?.doing).toBe("Starting");
		expect(said(line)).toBe("Started Fix the login bug on devbox");
	});

	it("falls back to the title it asked for before the output comes", () => {
		const line = agentToolLine(call("start_agent", { task: "x", title: "Fix it" }), names);
		expect(line?.target).toBe("Fix it");
	});

	it("counts the agents a wait is on, and says when it ran out", () => {
		const two = call("wait_agent", { agents: ["agent-a", "agent-b"] }, { timed_out: true });
		expect(agentToolLine(two, names)?.doing).toBe("Waiting on");
		expect(said(agentToolLine(two, names))).toBe("Waited on 2 agents (Timed out)");
		const one = call("wait_agent", { agents: ["agent-b"] });
		expect(said(agentToolLine(one, names))).toBe("Waited on Write the notes");
	});

	it("reads the rest", () => {
		const line = (name: string, input: object | null, output?: object) =>
			said(agentToolLine(call(name, input, output), names));
		expect(line("send_agent", { agent: "agent-a", text: "go" })).toBe(
			"Messaged Fix the login bug"
		);
		expect(line("interrupt_agent", { agent: "agent-a", stop: true })).toBe(
			"Stopped Fix the login bug"
		);
		expect(line("interrupt_agent", { agent: "gone" })).toBe("Interrupted an agent");
		expect(line("list_agents", {}, { agents: [{}, {}, {}] })).toBe("Listed 3 agents");
		expect(line("list_targets", null)).toBe("Checked where agents can start");
		expect(line("read_agent", { agent: "agent-b" })).toBe("Read Write the notes");
	});

	it("copes with input that isn't JSON", () => {
		const line = agentToolLine(
			{ server: "maxhq", name: "send_agent", input: "not json", output: null },
			names
		);
		expect(said(line)).toBe("Messaged an agent");
	});
});
