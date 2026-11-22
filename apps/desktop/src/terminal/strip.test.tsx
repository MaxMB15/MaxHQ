// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ServicesProvider, useView } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { Entity, Terminal } from "@maxhq/shared/bindings";
import { mockEntities } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";

import { TerminalStrip } from "./strip";

const AGENT = "agent-release";

/** The device the sample agent works on. */
const deviceOf = (entities: readonly Entity[]): string => {
	const agent = entities.find((e) => e.entity === "agent" && e.id === AGENT);
	const workspace =
		agent?.entity === "agent"
			? entities.find((e) => e.entity === "workspace" && e.id === agent.workspace)
			: undefined;
	if (workspace?.entity !== "workspace") throw new Error("no sample workspace");
	return workspace.device;
};

/** A drawer tab of the agent's, running a command unless `over` says otherwise. */
const tab = (id: string, device: string, over: Partial<Terminal> = {}): Entity => ({
	entity: "terminal",
	id,
	agent: AGENT,
	device,
	kind: { type: "shell" },
	moved: false,
	title: "zsh",
	cwd: "~/code/example-app",
	status: { state: "running" },
	busy: false,
	marks: true,
	password_prompt: false,
	started_at: 1,
	cols: 100,
	rows: 24,
	...over,
});

/** Says when the view has the tabs, so a test can look for a strip that isn't there. */
const Loaded = () => (useView((s) => s.view.tables.terminal.size > 0) ? <span>loaded</span> : null);

/**
 * The strip over the sample, with these tabs of the agent's, and its
 * device's maxd without terminals when `terminals` is false.
 */
const renderStrip = (
	tabs: (device: string) => readonly Entity[],
	{ terminals = true }: { terminals?: boolean } = {}
) => {
	const mock = mockSource({ now: () => NOW, tick: false });
	const sample = mockEntities(NOW);
	const device = deviceOf(sample);
	const snapshot = mock.snapshot.bind(mock);
	const source = Object.assign(mock, {
		snapshot: async () => {
			const state = await snapshot();
			const entities = state.entities.map((e) =>
				!terminals && e.entity === "device_link" && e.device === device
					? { ...e, capabilities: { ...e.capabilities, terminals: false } }
					: e
			);
			return { ...state, entities: [...entities, ...tabs(device)] };
		},
	});
	render(
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<TerminalStrip agent={AGENT} device={device} />
				<Loaded />
			</FixedNow>
		</ServicesProvider>
	);
};

const strip = () => document.querySelector("[data-terminal-strip]");

describe("the strip under the composer", () => {
	it("names the command a hidden tab runs and counts the tabs", async () => {
		renderStrip((device) => [
			tab("t1", device),
			tab("t2", device, {
				kind: { type: "run", command: "pnpm dev", private: false },
				moved: true,
			}),
		]);

		await waitFor(() => expect(strip()).not.toBeNull());
		expect(strip()).toHaveTextContent(/^Terminal·pnpm dev running·2 tabs/);
	});

	it("names the command a shell runs by its title", async () => {
		renderStrip((device) => [tab("t1", device, { busy: true, title: "vite" })]);

		await waitFor(() => expect(strip()).toHaveTextContent("vite running·1 tab"));
	});

	it("isn't there while every tab waits at its prompt", async () => {
		renderStrip((device) => [tab("t1", device)]);

		await screen.findByText("loaded");
		expect(strip()).toBeNull();
	});

	it("leaves out a run that's still a card in the timeline", async () => {
		renderStrip((device) => [
			tab("t1", device, {
				kind: { type: "run", command: "pnpm dev", private: false },
				moved: false,
			}),
		]);

		await screen.findByText("loaded");
		expect(strip()).toBeNull();
	});

	it("isn't there when the agent's device has no terminals", async () => {
		renderStrip((device) => [tab("t1", device, { busy: true, title: "vite" })], {
			terminals: false,
		});

		await screen.findByText("loaded");
		expect(strip()).toBeNull();
	});
});
