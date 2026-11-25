// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";
import { DockBadgeSection } from "@/settings/dock-badge-settings";
import { agent, entities } from "@maxhq/shared/test/build";
import { DEFAULT_BADGE_PREFS } from "@maxhq/shared/view/badge";
import { applySnapshot } from "@maxhq/shared/view/replica";
import { createViewStore } from "@maxhq/shared/view/store";

import { BADGE_KEY, overlayText, setBadgePrefs, watchDockBadge } from "./dock-badge";

const app = globalThis as { isTauri?: boolean };
const counts: (number | undefined)[] = [];
/** What each overlay set on the taskbar was: an image's size, or none. */
const overlays: (string | null)[] = [];
const images = new Map<number, string>();
const closed: number[] = [];

beforeEach(() => {
	app.isTauri = true;
	counts.length = 0;
	overlays.length = 0;
	images.clear();
	closed.length = 0;
	localStorage.clear();
	setBadgePrefs(DEFAULT_BADGE_PREFS);
	mockWindows("main");
	mockIPC((command, args?: InvokeArgs) => {
		if (command === "plugin:window|set_badge_count") {
			counts.push((args as { value?: number }).value);
		}
		if (command === "plugin:image|new") {
			const { rgba, width, height } = args as {
				rgba: number[];
				width: number;
				height: number;
			};
			const rid = images.size + 1;
			images.set(rid, `${width}x${height}:${rgba.length}`);
			return rid;
		}
		if (command === "plugin:window|set_overlay_icon") {
			const { value } = args as { value?: number | null };
			overlays.push(value == null ? null : (images.get(value) ?? "?"));
		}
		if (command === "plugin:resources|close") closed.push((args as { rid: number }).rid);
		return null;
	});
});

afterEach(() => {
	clearMocks();
	vi.restoreAllMocks();
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

describe("the taskbar overlay on Windows", () => {
	/** A canvas that draws nothing but hands back its pixels. */
	const fakeCanvas = () =>
		vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
			() =>
				({
					beginPath: () => undefined,
					arc: () => undefined,
					fill: () => undefined,
					fillText: () => undefined,
					getImageData: (_x: number, _y: number, width: number, height: number) => ({
						data: new Uint8ClampedArray(width * height * 4),
					}),
				}) as unknown as CanvasRenderingContext2D
		);

	it("draws the count in place of a badge, and clears at zero", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/131.0"
		);
		fakeCanvas();
		const view = createViewStore();
		const stop = watchDockBadge({ view } as Services);
		view.setState({ view: viewOf([{ state: "done_unread" }, { state: "idle" }]) });
		await settle();
		view.setState({ view: viewOf([{ state: "idle" }]) });
		await settle();
		expect(counts).toEqual([]);
		expect(overlays).toEqual([null, "32x32:4096", null]);
		expect(closed).toEqual([1]);
		stop();
	});

	it("says 9+ past nine, which is all that fits", () => {
		expect([0, 1, 9, 10, 250].map(overlayText)).toEqual(["0", "1", "9", "9+", "9+"]);
	});
});
