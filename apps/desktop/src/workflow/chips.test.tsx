// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkflowRun } from "@maxhq/shared/bindings";
import { MIN, NOW } from "@maxhq/shared/test/build";

import { WorkflowsEntry } from "./chips";

const run = (id: string, over: Partial<WorkflowRun> = {}): WorkflowRun => ({
	id: `wfr-${id}`,
	key: { device: "d1", project: "p1", workflow: "pipeline", run: id },
	label: id,
	progress: { done: 2, total: 5 },
	current: [],
	needs_you: false,
	orchestrators: [],
	last_activity: NOW - MIN,
	done: false,
	...over,
});

const entry = (folded: boolean) => {
	const onToggle = vi.fn();
	const onOpen = vi.fn();
	render(
		<WorkflowsEntry
			project="Example"
			runs={[run("Live run"), run("Ended run", { done: true, failed: true })]}
			folded={folded}
			showing={null}
			now={NOW}
			onToggle={onToggle}
			onOpen={onOpen}
			entryMenu={null}
			runMenu={() => null}
		/>
	);
	return { onToggle, onOpen };
};

describe("the Workflows entry", () => {
	it("folds to one row with the count", () => {
		const { onToggle } = entry(true);
		const header = screen.getByRole("button", { expanded: false });
		expect(header.textContent).toContain("2");
		expect(screen.queryByText("Live run")).toBeNull();
		fireEvent.click(header);
		expect(onToggle).toHaveBeenCalled();
	});

	it("shows progress for a live run and when a finished one ended", () => {
		const { onOpen } = entry(false);
		const live = screen.getByText("Live run").closest("button")!;
		expect(live.textContent).toContain("2/5");
		const ended = screen.getByText("Ended run").closest("button")!;
		expect(ended.textContent).not.toContain("2/5");
		expect(ended.querySelector("[role=img]")?.getAttribute("aria-label")).toMatch(/fail/i);
		fireEvent.click(live);
		expect(onOpen).toHaveBeenCalledWith("wfr-Live run");
	});
});
