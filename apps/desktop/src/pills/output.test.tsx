// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OutputRef } from "@maxhq/shared/pills/draft";
import { NOW } from "@maxhq/shared/test/build";

import { OutputPreview } from "./output-preview";
import { presentPill } from "./registry";
import { ShownPill } from "./shown";

// The app's backend, for the mask an agent's copy goes through.
const backend = vi.hoisted(() => ({
	inApp: false,
	mask: null as ((text: string) => Promise<string>) | null,
}));

vi.mock("@/native/platform", async (original) => ({
	...(await original<typeof import("@/native/platform")>()),
	inApp: () => backend.inApp,
}));

vi.mock("@/data/tauri-commands", async (original) => {
	const real = await original<typeof import("@/data/tauri-commands")>();
	return {
		...real,
		commands: {
			...real.commands,
			maskOutput: (text: string) => backend.mask!(text),
		},
	};
});

beforeEach(() => {
	backend.inApp = false;
	backend.mask = null;
});
afterEach(cleanup);

/** What `pnpm test` printed on the Mac: exit 1 after 4.2 seconds unless a test says otherwise. */
const output = (over: Partial<OutputRef> = {}): OutputRef => ({
	type: "output",
	command: "pnpm test",
	device_name: "Mac",
	cwd: "~/code/example-app",
	exit_code: 1,
	running: false,
	ran_at: NOW,
	duration_ms: 4200,
	lines_total: 250,
	text: "FAIL src/cache.test.ts\nTests 1 failed",
	...over,
});

const pill = () => document.querySelector<HTMLElement>(".pill")!;

/** The pill as the composer shows it, its preview the one a test gives. */
const show = (ref: OutputRef, editable = true) =>
	render(
		<ShownPill
			presentation={presentPill(
				{
					kind: "output",
					ref,
					preview: <OutputPreview output={ref} editable={editable} />,
				},
				{ now: NOW }
			)}
			selected={false}
		/>
	);

const hover = async () => {
	fireEvent.pointerEnter(pill(), { pointerType: "mouse" });
	fireEvent.mouseEnter(pill());
	fireEvent.mouseMove(pill());
	return waitFor(() => screen.getByRole("dialog"), { timeout: 2000 });
};

describe("an output pill", () => {
	it("is red when the command failed, with its exit code after it", () => {
		show(output());

		expect(pill()).toHaveAttribute("data-family", "output-failed");
		expect(pill()).toHaveAccessibleName("Output of pnpm test, exit 1");
		expect(pill()).toHaveTextContent(/pnpm test\s*exit 1$/);
	});

	it("is gray when the command succeeded", () => {
		show(output({ exit_code: 0 }));

		expect(pill()).toHaveAttribute("data-family", "output");
		expect(pill()).toHaveAccessibleName("Output of pnpm test, exit 0");
	});

	it("is gray with a spinner while the command runs", () => {
		show(output({ running: true, exit_code: null, duration_ms: null }));

		expect(pill()).toHaveAttribute("data-family", "output");
		expect(pill()).toHaveAccessibleName("Output of pnpm test, running");
		expect(pill().querySelector(".pill-spin")).not.toBeNull();
	});

	it("names the lines picked from a terminal instead of how it ended, and stays gray", () => {
		show(output({ lines: { first: 3, last: 8 }, terminal: "t1" }));

		expect(pill()).toHaveAttribute("data-family", "output");
		expect(pill()).toHaveAccessibleName("Output of pnpm test, 6 lines");
	});

	it("puts a command of several lines on one", () => {
		show(output({ command: "pnpm install\n  pnpm test", exit_code: 0 }));

		expect(pill()).toHaveAccessibleName("Output of pnpm install pnpm test, exit 0");
	});
});

describe("its preview", () => {
	it("shows the command, how it ended and exactly the text the agent gets", async () => {
		show(output());

		const preview = await hover();

		expect(preview).toHaveTextContent("Output of pnpm test");
		expect(preview).toHaveTextContent("exit 1 · 250 lines · Mac");
		expect(preview.querySelector("pre")).toHaveTextContent(
			"FAIL src/cache.test.ts Tests 1 failed"
		);
		expect(preview).toHaveTextContent(
			"The agent gets the last 200 lines. MaxHQ masks keys and tokens it recognizes. Backspace the pill to keep it private."
		);
	});

	it("leaves out how to keep it back once the message has gone", async () => {
		show(output(), false);

		const preview = await hover();

		expect(preview).toHaveTextContent("Output of pnpm test");
		expect(preview).not.toHaveTextContent("Backspace the pill");
	});

	it("says when there's no output yet", async () => {
		show(output({ running: true, exit_code: null, text: "", lines_total: 0 }));

		expect(await hover()).toHaveTextContent("No output yet");
	});

	it("shows the text masked the way the app masks it, and nothing until it has", async () => {
		backend.inApp = true;
		let answer: (masked: string) => void = () => {};
		backend.mask = vi.fn(
			() =>
				new Promise<string>((resolve) => {
					answer = resolve;
				})
		);
		show(output({ text: "API_KEY=example-key-value" }));

		const preview = await hover();
		expect(backend.mask).toHaveBeenCalledWith("API_KEY=example-key-value");
		expect(preview.querySelector("pre")?.textContent).toBe("");

		await act(async () => answer("API_KEY=********"));

		expect(preview.querySelector("pre")?.textContent).toBe("API_KEY=********");
		expect(preview).not.toHaveTextContent("example-key-value");
	});
});
