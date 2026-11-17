// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ActionResult, DataSource } from "../data/source";
import { project } from "../test/build";
import { createViewStore } from "../view/store";

import { setProjectHidden } from "./hide-project";
import { createInboxNotices } from "./inbox-notice";

const setup = (answer: ActionResult) => {
	const calls: [string, boolean][] = [];
	const source = {
		setProjectHidden: async (id: string, hidden: boolean) => {
			calls.push([id, hidden]);
			return answer;
		},
	} as unknown as DataSource;
	const notices = createInboxNotices(source, createViewStore());
	return { calls, notices };
};

const api = project("p1", "api");

describe("setProjectHidden", () => {
	it("asks to hide or show the project and stays quiet when it goes through", async () => {
		const { calls, notices } = setup({ status: "ok", data: { status: "done" } });
		await setProjectHidden(notices, api, true);
		await setProjectHidden(notices, api, false);
		expect(calls).toEqual([
			["p1", true],
			["p1", false],
		]);
		expect(notices.store.getState().failures).toEqual([]);
	});

	it("says what went wrong in the list's notice", async () => {
		const { notices } = setup({
			status: "error",
			error: { kind: "failed", message: "the disk is full" },
		});
		await setProjectHidden(notices, api, true);
		expect(notices.store.getState().failures).toEqual(["Couldn't hide api. The disk is full."]);
	});
});
