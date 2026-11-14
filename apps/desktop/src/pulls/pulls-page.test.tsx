// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { MOCK_PULLS_PROJECT, MOCK_REPO } from "@maxhq/shared/data/mock-pulls";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { summary } from "@maxhq/shared/test/pulls";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = (source: DataSource = mockSource({ now: () => NOW, tick: false })) => {
	const ui = createUiStore();
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source, ui };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });

/** Opens the MaxHQ project's Pulls page from the sidebar. */
const openPage = async () => {
	const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
	fireEvent.click(await within(maxhq).findByRole("button", { name: /^Pull requests/ }));
	return screen.findByRole("heading", { level: 1, name: "Pull requests" });
};

const numbers = async (name: string) => {
	const list = await screen.findByRole("list", { name });
	return within(list)
		.getAllByRole("listitem")
		.map((row) => /#(\d+)/.exec(row.textContent ?? "")?.[1]);
};

describe("the Pulls entry", () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	const noPulls = { status: "ok" as const, data: { status: "pulls" as const, pulls: [] } };
	const entryIn = (region: HTMLElement) =>
		within(region).queryByRole("button", { name: /^Pull requests/ });

	it("hides on a project with no PRs at all, and shows within a minute of one opening", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const source = mockSource({ now: () => NOW, tick: false });
		const list = vi.spyOn(source, "listPulls").mockResolvedValue(noPulls);
		renderApp(source);
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		await waitFor(() => expect(list).toHaveBeenCalledWith(MOCK_PULLS_PROJECT, "closed"));
		await act(() => vi.advanceTimersByTimeAsync(0));
		expect(entryIn(maxhq)).not.toBeInTheDocument();

		list.mockRestore();
		await act(() => vi.advanceTimersByTimeAsync(60_000));
		await waitFor(() => expect(entryIn(maxhq)).toBeInTheDocument());
	});

	it("shows on a project whose PRs are all merged or closed", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const read = source.listPulls.bind(source);
		const list = vi
			.spyOn(source, "listPulls")
			.mockImplementation((project, state) =>
				state === "open" ? Promise.resolve(noPulls) : read(project, state)
			);
		renderApp(source);
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		await waitFor(() => expect(entryIn(maxhq)).toBeInTheDocument());
		expect(list).toHaveBeenCalledWith(MOCK_PULLS_PROJECT, "closed");
	});

	it("reads only the open list while a PR is open", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const list = vi.spyOn(source, "listPulls");
		renderApp(source);
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		await waitFor(() => expect(entryIn(maxhq)).toBeInTheDocument());
		expect(list).not.toHaveBeenCalledWith(MOCK_PULLS_PROJECT, "closed");
	});

	it("stays after a read fails, since its page says why", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		vi.spyOn(source, "listPulls").mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "GitHub is down." },
		});
		renderApp(source);
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		await waitFor(() => expect(entryIn(maxhq)).toBeInTheDocument());
	});

	it("stays while its page is open after the last PR closes, then goes", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const source = mockSource({ now: () => NOW, tick: false });
		const list = vi.spyOn(source, "listPulls");
		const { ui } = renderApp(source);
		await openPage();
		const maxhq = within(nav()).getByRole("region", { name: "MaxHQ" });

		list.mockResolvedValue(noPulls);
		await act(() => vi.advanceTimersByTimeAsync(60_000));
		expect(await screen.findByText("No open pull requests")).toBeInTheDocument();
		expect(entryIn(maxhq)).toHaveAttribute("aria-current", "page");

		act(() => ui.getState().selectAgent("agent-bindings", false));
		await waitFor(() => expect(entryIn(maxhq)).not.toBeInTheDocument());
		expect(within(nav()).getByRole("region", { name: "MaxHQ" })).toBe(maxhq);
	});

	it("shows on a project a GitHub App reaches, with how many are ready to merge", async () => {
		renderApp();
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		const entry = await within(maxhq).findByRole("button", { name: /^Pull requests/ });
		expect(await within(entry).findByLabelText("1 ready to merge")).toHaveTextContent("1");

		const comet = within(nav()).getByRole("region", { name: "Comet" });
		expect(
			within(comet).queryByRole("button", { name: /^Pull requests/ })
		).not.toBeInTheDocument();
	});
});

describe("the project's menu", () => {
	const noPulls = { status: "ok" as const, data: { status: "pulls" as const, pulls: [] } };

	it("opens the Pulls page from a right-click on a project with no PRs", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const list = vi.spyOn(source, "listPulls").mockResolvedValue(noPulls);
		renderApp(source);
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		await waitFor(() => expect(list).toHaveBeenCalledWith(MOCK_PULLS_PROJECT, "closed"));
		expect(within(maxhq).queryByRole("button", { name: /^Pull requests/ })).toBeNull();

		fireEvent.contextMenu(within(maxhq).getByRole("button", { name: /^MaxHQ/ }));
		fireEvent.click(await screen.findByRole("menuitem", { name: "Pull requests" }));
		expect(await screen.findByText("No open pull requests")).toBeInTheDocument();
		expect(within(maxhq).getByRole("button", { name: /^Pull requests/ })).toHaveAttribute(
			"aria-current",
			"page"
		);
	});

	it("offers the same items from the right-click and the ⋯", async () => {
		renderApp();
		const maxhq = await within(nav()).findByRole("region", { name: "MaxHQ" });
		const names = async () =>
			(await screen.findAllByRole("menuitem")).map((item) => item.textContent);

		fireEvent.contextMenu(within(maxhq).getByRole("button", { name: /^MaxHQ/ }));
		const fromRightClick = await names();
		expect(fromRightClick).toContain("Pull requests");
		expect(fromRightClick).toContain("Project settings…");
		fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

		fireEvent.click(within(maxhq).getByRole("button", { name: "More for MaxHQ" }));
		expect(await names()).toEqual(fromRightClick);
	});

	it("leaves Pull requests out where no GitHub App reaches the project", async () => {
		renderApp();
		const comet = await within(nav()).findByRole("region", { name: "Comet" });
		fireEvent.contextMenu(within(comet).getByRole("button", { name: /^Comet/ }));
		const names = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
		expect(names).toContain("Project settings…");
		expect(names).not.toContain("Pull requests");
	});
});

describe("the Pulls page", () => {
	it("lists the open PRs by merge readiness, then by another sort", async () => {
		renderApp();
		await openPage();
		expect(await numbers("Open pull requests")).toEqual(["141", "143", "145", "142", "144"]);

		const row = within(screen.getByRole("list", { name: "Open pull requests" }))
			.getAllByRole("listitem")
			.at(0)!;
		expect(within(row).getByText("Show the week under the 5-hour bar")).toBeInTheDocument();
		expect(within(row).getByText("sam-reviewer")).toBeInTheDocument();
		expect(within(row).getByRole("img", { name: "Checks passing" })).toBeInTheDocument();
		expect(within(row).getByRole("img", { name: "Approved" })).toBeInTheDocument();
		expect(within(row).getByText("+12")).toBeInTheDocument();
		expect(within(row).getByText("25m")).toBeInTheDocument();
		expect(within(row).getByRole("button", { name: "Release checklist" })).toBeInTheDocument();

		const draft = within(screen.getByRole("list", { name: "Open pull requests" }))
			.getAllByRole("listitem")
			.at(1)!;
		expect(within(draft).getByText("Draft")).toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: "Sort: Merge readiness" }));
		fireEvent.click(await screen.findByRole("menuitemradio", { name: "Recently updated" }));
		expect(await numbers("Open pull requests")).toEqual(["145", "142", "141", "143", "144"]);
	});

	it("shows the closed PRs on their tab", async () => {
		renderApp();
		await openPage();
		fireEvent.click(screen.getByRole("tab", { name: "Closed" }));
		expect(await numbers("Closed pull requests")).toEqual(["138", "136"]);
		expect(screen.getByText("Try a tray icon")).toBeInTheDocument();
	});

	it("opens a linked agent, and goes back with Back", async () => {
		const { ui } = renderApp();
		await openPage();
		const [first] = within(
			await screen.findByRole("list", { name: "Open pull requests" })
		).getAllByRole("listitem");
		fireEvent.click(within(first!).getByRole("button", { name: "Release checklist" }));
		expect(ui.getState().pullsOf).toBeNull();
		expect(screen.queryByRole("heading", { name: "Pull requests" })).not.toBeInTheDocument();

		await openPage();
		fireEvent.click(screen.getByRole("button", { name: "Back" }));
		expect(ui.getState().pullsOf).toBeNull();
	});

	it("opens a row's PR in the viewer, on a device it's linked to", async () => {
		const { ui } = renderApp();
		await openPage();
		fireEvent.click(await screen.findByRole("button", { name: /#142/ }));
		expect(ui.getState().shownPull?.key).toEqual({ repo: MOCK_REPO, number: 142 });
		expect(
			await screen.findByRole("complementary", { name: "Pull request #142" })
		).toBeInTheDocument();
	});

	it("says when there are none, and why a read failed", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const list = vi.spyOn(source, "listPulls");
		list.mockResolvedValue({ status: "ok", data: { status: "pulls", pulls: [] } });
		const { ui } = renderApp(source);
		act(() => ui.getState().openPulls(MOCK_PULLS_PROJECT));
		expect(await screen.findByText("No open pull requests")).toBeInTheDocument();

		list.mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "GitHub is down." },
		});
		fireEvent.click(screen.getByRole("tab", { name: "Closed" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't read the pull requests. GitHub is down."
		);
	});

	it("asks for the App's new permissions when checks read as unknown", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const row = summary(7, { checks: "unknown" });
		vi.spyOn(source, "listPulls").mockResolvedValue({
			status: "ok",
			data: { status: "pulls", pulls: [{ ...row, key: { repo: MOCK_REPO, number: 7 } }] },
		});
		renderApp(source);
		await openPage();
		const banner = await screen.findByRole("note");
		expect(banner).toHaveTextContent(
			"Accept the new permissions for your GitHub App on example-user to see checks here"
		);
		expect(within(banner).getByRole("link", { name: "Accept" })).toHaveAttribute(
			"href",
			"https://github.com/apps/maxhq-workspaces/installations/new"
		);
	});
});
