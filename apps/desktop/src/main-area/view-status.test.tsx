// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { retryNote } from "./view-status";

/** The sample source, with its snapshot swapped for `snapshot`. */
const sourceWith = (snapshot: (real: DataSource["snapshot"]) => DataSource["snapshot"]) => {
	const source = mockSource({ now: () => NOW, tick: false });
	return Object.assign(source, { snapshot: snapshot(source.snapshot.bind(source)) });
};

describe("the main area before the view loads", () => {
	it("counts down to the next try", () => {
		expect(retryNote(NOW + 5000, NOW)).toBe("Trying again in 5 s.");
		expect(retryNote(NOW + 4200, NOW)).toBe("Trying again in 5 s.");
		expect(retryNote(NOW, NOW)).toBe("Trying again now.");
		expect(retryNote(null, NOW)).toBe("Trying again now.");
	});

	it("shows a spinner while it loads", async () => {
		render(<App source={sourceWith(() => () => new Promise(() => {}))} ui={createUiStore()} />);
		const main = screen.getByRole("main");
		expect(await within(main).findByRole("status")).toHaveTextContent("Loading your agents");
	});

	it("says why it failed and when it tries again, and Retry tries now", async () => {
		const snapshot = vi.fn<DataSource["snapshot"]>();
		const source = sourceWith((real) => {
			snapshot
				.mockRejectedValueOnce(new Error("maxd isn't running."))
				.mockImplementation(real);
			return snapshot;
		});
		render(<App source={source} ui={createUiStore()} />);
		const main = screen.getByRole("main");
		expect(await within(main).findByRole("alert")).toHaveTextContent(
			"Couldn't load your agents. maxd isn't running."
		);
		expect(within(main).getByText(/^Trying again in \d+ s\.$/)).toBeInTheDocument();

		fireEvent.click(within(main).getByRole("button", { name: "Retry" }));
		expect(snapshot).toHaveBeenCalledTimes(2);
		expect(
			await screen.findByRole("navigation", { name: "Agents" }, { timeout: 3000 })
		).toBeInTheDocument();
		expect(within(main).queryByText(/Couldn't load your agents/)).not.toBeInTheDocument();
	});
});
