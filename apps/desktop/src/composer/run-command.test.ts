// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import type { ActionResult } from "@maxhq/shared/data/source";
import { JUST_RAN_MS } from "@maxhq/shared/terminal/handoff";
import type { ViewState } from "@maxhq/shared/view/replica";

import { forgetNewest, opensPanelNow, startRun, startSessionRun } from "./run-command";

afterEach(forgetNewest);

const SIZE = { cols: 100, rows: 24 };

// A private command puts no output pill in, so starting one never reads the view.
const RUN = { agent: "agent-1", command: "git commit", private: true, size: SIZE } as const;

const noView = (): ViewState => {
	throw new Error("A private command doesn't read the view");
};

/** A device whose session runs the command as `block`. */
const session = (block: number) => ({
	sessionRun: async (): Promise<ActionResult> => ({
		status: "ok",
		data: { status: "session_ran", terminal: "s1", item: "card-1", block },
	}),
});

/** A device that runs the command in a shell of its own. */
const fresh = {
	openTerminal: async (): Promise<ActionResult> => ({
		status: "ok",
		data: { status: "terminal_opened", terminal: "run-7" },
	}),
};

describe("the newest command started here", () => {
	it("opens the panel once, when its program first goes full screen", async () => {
		await startSessionRun(session(4), noView, { ...RUN, fresh: false });

		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(true);
		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(false);
	});

	it("opens the panel for a command in a shell of its own", async () => {
		await startRun(fresh, noView, RUN);

		expect(opensPanelNow({ terminal: "run-7", block: null }, true)).toBe(true);
	});

	it("leaves the panel alone for any other command, keeping its turn", async () => {
		await startSessionRun(session(4), noView, { ...RUN, fresh: false });

		expect(opensPanelNow({ terminal: "s1", block: 3 }, true)).toBe(false);
		expect(opensPanelNow({ terminal: "run-7", block: null }, true)).toBe(false);
		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(true);
	});

	it("is only the newest: a later start takes its place", async () => {
		await startSessionRun(session(4), noView, { ...RUN, fresh: false });
		await startSessionRun(session(5), noView, { ...RUN, fresh: false });

		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(false);
		expect(opensPanelNow({ terminal: "s1", block: 5 }, true)).toBe(true);
	});

	it("leaves the panel alone more than ten seconds after the start", async () => {
		await startSessionRun(session(4), noView, { ...RUN, fresh: false });

		expect(
			opensPanelNow({ terminal: "s1", block: 4 }, true, Date.now() + JUST_RAN_MS + 1)
		).toBe(false);
	});

	it("leaves the panel alone while you type elsewhere, and doesn't ask again", async () => {
		await startSessionRun(session(4), noView, { ...RUN, fresh: false });

		expect(opensPanelNow({ terminal: "s1", block: 4 }, false)).toBe(false);
		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(false);
	});

	it("leaves the panel alone when this window started nothing", () => {
		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(false);
	});

	it("leaves the panel alone when the command didn't start", async () => {
		const away = {
			sessionRun: async (): Promise<ActionResult> => ({
				status: "ok",
				data: { status: "refused", reason: "The device is away" },
			}),
		};
		await startSessionRun(away, noView, { ...RUN, fresh: false });

		expect(opensPanelNow({ terminal: "s1", block: 4 }, true)).toBe(false);
	});
});
