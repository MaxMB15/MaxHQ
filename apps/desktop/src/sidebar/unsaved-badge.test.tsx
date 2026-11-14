// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { Workspace } from "@maxhq/shared/bindings";
import { MHQ_WORKSPACE, mockEntities } from "@maxhq/shared/data/mock-view";
import { NOW } from "@maxhq/shared/test/build";

import { UnsavedBadge } from "./devices";

const made = (): Workspace => {
	const found = mockEntities(NOW).find(
		(e): e is Workspace & { entity: "workspace" } =>
			e.entity === "workspace" && e.id === MHQ_WORKSPACE
	);
	if (!found) throw new Error("No made workspace in the mock");
	return found;
};

describe("the unsaved count in the sidebar", () => {
	it("counts the unsaved work and says what it is", () => {
		render(<UnsavedBadge workspace={made()} now={NOW} />);
		const badge = screen.getByRole("img", {
			name: /^Unsaved: 3 changed files and 1 unpushed commit\./,
		});
		expect(badge).toHaveTextContent("4");
		expect(badge.className).not.toContain("bg-border");
	});

	it("dims the count of a stopped workspace, whose count may be old", () => {
		const workspace = made();
		const stopped: Workspace = {
			...workspace,
			managed: { ...workspace.managed!, state: { state: "stopped" } },
		};
		render(<UnsavedBadge workspace={stopped} now={NOW} />);
		expect(screen.getByRole("img", { name: /^Unsaved: / }).className).toContain("bg-border");
	});

	it("shows nothing when nothing is unsaved", () => {
		const workspace = made();
		const clean: Workspace = {
			...workspace,
			managed: {
				...workspace.managed!,
				unsaved: { changed: 0, commits: 0, no_upstream: 0, read_at: NOW },
			},
		};
		const { container } = render(<UnsavedBadge workspace={clean} now={NOW} />);
		expect(container).toBeEmptyDOMElement();
	});
});
