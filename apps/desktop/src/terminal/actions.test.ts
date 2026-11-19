// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";
import type { SessionBlock, Terminal } from "@maxhq/shared/bindings";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import type { TerminalEntity } from "@maxhq/shared/terminal/select";
import type { UserSessionItem } from "@maxhq/shared/terminal/session";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { applySnapshot } from "@maxhq/shared/view/replica";
import { createViewStore } from "@maxhq/shared/view/store";

import { restartTab } from "./actions";
import { drawersOf } from "./drawer-prefs";

const AGENT = "agent-1";
const FOLDER = "/Users/dev/code/example-app";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };
const opened = (terminal: string): ActionResult => ({
	status: "ok",
	data: { status: "terminal_opened", terminal },
});

/** A session terminal for the agent, running unless a test says otherwise. */
const session = (id: string, over: Partial<Terminal> = {}): TerminalEntity => ({
	entity: "terminal",
	id,
	agent: AGENT,
	device: "mac",
	kind: { type: "session" },
	moved: false,
	title: "zsh",
	cwd: FOLDER,
	status: { state: "running" },
	busy: false,
	marks: true,
	password_prompt: false,
	started_at: 1,
	cols: 100,
	rows: 24,
	...over,
});

const EXITED = { state: "exited", code: 0, signal: null } as const;

const block = (id: number, over: Partial<SessionBlock>): SessionBlock => ({
	id,
	command: "true",
	cwd: FOLDER,
	cwd_after: null,
	private: false,
	process: EXITED,
	started_at: 1,
	duration_ms: 10,
	lines: 0,
	output: "",
	output_truncated: false,
	secret_prompts: 0,
	start: null,
	...over,
});

/** The services an exited tab's Start again reaches, with these terminals and cards. */
const setup = (terminals: readonly TerminalEntity[], cards: readonly UserSessionItem[]) => {
	const calls = {
		sessionCd: vi.fn<DataSource["sessionCd"]>(async () => opened("s-new")),
		closeTerminal: vi.fn<DataSource["closeTerminal"]>(async () => DONE),
	};
	const source = {
		...calls,
		openTimeline: async (_agent, onUpdate) => {
			onUpdate({ type: "reset", seq: 1, items: [...cards], has_older: false }, false);
			return () => {};
		},
	} as Partial<DataSource> as DataSource;
	const view = createViewStore();
	view.setState({
		view: applySnapshot({ seq: 1, entities: [...terminals] }),
	});
	const services = { source, view } as Partial<Services> as Services;
	return { services, ...calls };
};

const card = (blocks: readonly SessionBlock[]): UserSessionItem =>
	timelineItem(
		{
			kind: "user_session",
			terminal: "s-old",
			device: "mac",
			blocks: [...blocks],
			sealed: true,
			sent: false,
		},
		{ id: "card-1", agent: AGENT, ordinal: 1, started_at: 1 }
	) as UserSessionItem;

afterEach(() => {
	localStorage.clear();
});

describe("starting an exited session tab again", () => {
	it("opens the session in the folder its last command left it in", async () => {
		const old = session("s-old", { status: EXITED });
		const { services, sessionCd, closeTerminal } = setup(
			[old],
			[
				card([
					block(1, { command: "cd apps/web", cwd_after: `${FOLDER}/apps/web` }),
					block(2, { command: "pnpm test", cwd: `${FOLDER}/apps/web` }),
				]),
			]
		);

		await restartTab(services, old);

		expect(sessionCd).toHaveBeenCalledExactlyOnceWith(AGENT, `${FOLDER}/apps/web`);
		expect(drawersOf(services).getState().active[AGENT]).toBe("s-new");
		expect(closeTerminal).toHaveBeenCalledExactlyOnceWith("s-old", true);
	});

	it("opens it where the shell started when it ran nothing", async () => {
		const old = session("s-old", { status: EXITED, cwd: `${FOLDER}/docs` });
		const { services, sessionCd } = setup([old], []);

		await restartTab(services, old);

		expect(sessionCd).toHaveBeenCalledExactlyOnceWith(AGENT, `${FOLDER}/docs`);
	});

	it("shows the session that runs instead, as the agent keeps only one", async () => {
		const old = session("s-old", { status: EXITED });
		const live = session("s-live", { started_at: 2 });
		const { services, sessionCd, closeTerminal } = setup([old, live], []);
		drawersOf(services).setState({ split: { [AGENT]: "s-old" } });

		await restartTab(services, old);

		expect(sessionCd).not.toHaveBeenCalled();
		const drawers = drawersOf(services).getState();
		expect(drawers.active[AGENT]).toBe("s-live");
		expect(drawers.split[AGENT]).toBeUndefined();
		expect(drawers.focusing).toBe(AGENT);
		expect(closeTerminal).toHaveBeenCalledExactlyOnceWith("s-old", true);
	});
});
