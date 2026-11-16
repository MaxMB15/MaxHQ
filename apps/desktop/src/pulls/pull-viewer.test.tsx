// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { PullFile } from "@maxhq/shared/bindings";
import { MOCK_REPO, SIGN_IN_MS } from "@maxhq/shared/data/mock-pulls";
import { MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { withHeaders } from "./tabs";

afterEach(() => {
	vi.useRealTimers();
});

const renderApp = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const ui = createUiStore();
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source, ui };
};

/** Opens PR `number` from the MaxHQ project's Pulls page, and gives its panel. */
const openPull = async (number: number) => {
	const nav = screen.getByRole("navigation", { name: "Agents" });
	const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
	fireEvent.click(await within(maxhq).findByRole("button", { name: /^Pull requests/ }));
	fireEvent.click(await screen.findByRole("button", { name: new RegExp(`^#${number}(?!\\d)`) }));
	return screen.findByRole("complementary", { name: `Pull request #${number}` });
};

const tab = (panel: HTMLElement, name: string) =>
	fireEvent.click(within(panel).getByRole("tab", { name }));

describe("the PR viewer", () => {
	it("shows the PR, and sends you to GitHub until you sign in", async () => {
		renderApp();
		const panel = await openPull(141);
		expect(
			await within(panel).findByRole("heading", {
				name: /Show the week under the 5-hour bar/,
			})
		).toBeInTheDocument();
		expect(within(panel).getByText("sam-reviewer")).toBeInTheDocument();
		expect(within(panel).getByText("usage-week into main")).toBeInTheDocument();
		expect(within(panel).getAllByRole("link", { name: /Open on GitHub/ })[0]).toHaveAttribute(
			"href",
			`https://github.com/${MOCK_REPO}/pull/141`
		);

		const reviews = await within(panel).findByRole("list", { name: "Reviews" });
		expect(within(reviews).getByText("alex-maintainer").parentElement).toHaveTextContent(
			"Approved"
		);
		expect(within(panel).getByText("No conflicts with main.")).toBeInTheDocument();
		expect(
			within(panel).getByText("Sign in to GitHub in Settings to merge here.")
		).toBeInTheDocument();
		expect(within(panel).queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
	});

	it("merges the PR at its head after you sign in, with the method you pick", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { source } = renderApp();
		const merge = vi.spyOn(source, "pullAction");
		const panel = await openPull(141);
		await within(panel).findByText("Sign in to GitHub in Settings to merge here.");

		await act(async () => {
			await source.beginGithubSignIn(MAC, null);
			await vi.advanceTimersByTimeAsync(SIGN_IN_MS);
		});
		const approve = await within(panel).findByRole("button", { name: "Approve" });
		expect(approve).toBeEnabled();

		fireEvent.click(within(panel).getByRole("button", { name: "Merge method" }));
		fireEvent.click(
			await screen.findByRole("menuitemradio", { name: /Create a merge commit/ })
		);
		fireEvent.click(
			await within(panel).findByRole("button", { name: "Create a merge commit" })
		);
		expect(await within(panel).findByText("Merged.")).toBeInTheDocument();
		expect(merge).toHaveBeenCalledWith(
			MOCK_REPO,
			141,
			{ action: "merge", method: "merge" },
			expect.any(String)
		);
	});

	it("says why a draft can't merge", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { source } = renderApp();
		const panel = await openPull(143);
		await act(async () => {
			await source.beginGithubSignIn(MAC, null);
			await vi.advanceTimersByTimeAsync(SIGN_IN_MS);
		});
		expect(
			await within(panel).findByText("Mark this draft ready for review before merging.")
		).toBeInTheDocument();
		expect(within(panel).getByRole("button", { name: "Squash and merge" })).toBeDisabled();
	});

	it("pages through the files", async () => {
		renderApp();
		const panel = await openPull(145);
		tab(panel, "Files");
		const files = await within(panel).findByRole("list", { name: "Files" });
		expect(within(files).getAllByRole("listitem")).toHaveLength(3);
		fireEvent.click(within(panel).getByRole("button", { name: "Show more files" }));
		expect(await within(panel).findByText("crates/daemon/Cargo.toml")).toBeInTheDocument();
		expect(within(files).getAllByRole("listitem")).toHaveLength(5);
		expect(within(panel).getAllByText("GitHub shows no diff for this file.")).toHaveLength(2);
		expect(
			within(panel).queryByRole("button", { name: "Show more files" })
		).not.toBeInTheDocument();
	});

	it("groups the checks by how each stands", async () => {
		renderApp();
		const panel = await openPull(142);
		tab(panel, "Checks");
		const failing = await within(panel).findByRole("region", { name: "Failing" });
		expect(failing).toHaveTextContent("Unit tests");
		expect(within(panel).getByRole("region", { name: "Passing" })).toHaveTextContent("Lint");
		expect(within(panel).queryByRole("region", { name: "Pending" })).not.toBeInTheDocument();
	});

	it("shows the conversation, oldest first", async () => {
		renderApp();
		const panel = await openPull(141);
		tab(panel, "Conversation");
		const comments = await within(panel).findByRole("list", { name: "Comments" });
		const rows = within(comments).getAllByRole("listitem");
		expect(rows).toHaveLength(3);
		expect(rows[0]).toHaveTextContent("Should the week bar hide on Cursor?");
		expect(within(panel).getByRole("link", { name: /Reply on GitHub/ })).toBeInTheDocument();
	});

	it("closes with its button and with Escape", async () => {
		const { ui } = renderApp();
		const panel = await openPull(141);
		fireEvent.click(within(panel).getByRole("button", { name: "Close the pull request" }));
		expect(ui.getState().shownPull).toBeNull();
		expect(panel).not.toBeInTheDocument();

		fireEvent.click(await screen.findByRole("button", { name: /^#142(?!\d)/ }));
		await screen.findByRole("complementary", { name: "Pull request #142" });
		fireEvent.keyDown(window, { key: "Escape" });
		expect(ui.getState().shownPull).toBeNull();
	});
});

describe("withHeaders", () => {
	const file = (status: PullFile["status"]): PullFile & { patch: string } => ({
		path: "src/a.ts",
		status,
		additions: 1,
		deletions: 0,
		patch: "@@ -0,0 +1 @@\n+a",
	});

	it("names the file on both sides, or /dev/null for an added or deleted one", () => {
		expect(withHeaders(file("modified"))).toBe(
			"--- a/src/a.ts\n+++ b/src/a.ts\n@@ -0,0 +1 @@\n+a"
		);
		expect(withHeaders(file("added")).split("\n")[0]).toBe("--- /dev/null");
		expect(withHeaders(file("removed")).split("\n")[1]).toBe("+++ /dev/null");
	});
});
