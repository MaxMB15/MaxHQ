// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { Services } from "@/app/services";
import { DockBadgeSection } from "@/settings/dock-badge-settings";
import { agent, entities } from "@maxhq/shared/test/build";
import { DEFAULT_BADGE_PREFS } from "@maxhq/shared/view/badge";
import { applySnapshot } from "@maxhq/shared/view/replica";
import { createViewStore } from "@maxhq/shared/view/store";

import { BADGE_KEY, setBadgePrefs, watchDockBadge } from "./dock-badge";

const app = globalThis as { isTauri?: boolean };
const counts: (number | undefined)[] = [];

beforeEach(() => {
	app.isTauri = true;
	counts.length = 0;
	localStorage.clear();
	setBadgePrefs(DEFAULT_BADGE_PREFS);
	mockWindows("main");
	mockIPC((command, args?: InvokeArgs) => {
		if (command === "plugin:window|set_badge_count") {
			counts.push((args as { value?: number }).value);
		}
		return null;
	});
});

afterEach(() => {
	clearMocks();
	delete app.isTauri;
});

const settle = () => act(() => new Promise((done) => setTimeout(done, 0)));

const viewOf = (states: Parameters<typeof agent>[0]["activity"][]) =>
	applySnapshot({
		seq: 1,
		entities: entities({
			agents: states.map((activity, i) => agent({ id: `a${i}`, activity })),
		}),
	});

describe("the Dock badge", () => {
	it("follows the view and the settings, and clears at zero", async () => {
		const view = createViewStore();
		const stop = watchDockBadge({ view } as Services);
		view.setState({ view: viewOf([{ state: "done_unread" }, { state: "idle" }]) });
		view.setState({
			view: viewOf([{ state: "done_unread" }, { state: "error", reason: "exit 1" }]),
		});
		await settle();
		// Nothing counts before the view loads, so the badge starts cleared.
		expect(counts).toEqual([undefined, 1, 2]);

		render(<DockBadgeSection />);
		await act(async () => {
			fireEvent.click(
				screen.getByRole("switch", { name: "Badge: Failed or stopped with an error" })
			);
		});
		await act(async () => {
			fireEvent.click(screen.getByRole("switch", { name: "Show the badge" }));
		});
		await settle();
		expect(counts).toEqual([undefined, 1, 2, 1, undefined]);
		expect(JSON.parse(localStorage.getItem(BADGE_KEY)!)).toMatchObject({
			show: false,
			counts: ["waiting", "finished"],
		});
		expect(screen.getByRole("switch", { name: "Badge: Finished" })).toHaveAttribute(
			"data-disabled"
		);
		stop();
	});
});
