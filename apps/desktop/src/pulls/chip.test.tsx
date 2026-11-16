// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Shell } from "@/App";
import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { MOCK_REPO } from "@maxhq/shared/data/mock-pulls";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const show = (agent: string) => {
	const source = mockSource({ now: () => NOW, tick: false });
	const ui = createUiStore({ selectedAgent: agent });
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={ui}>
				<Shell />
			</ServicesProvider>
		</FixedNow>
	);
	return { source, ui };
};

const header = () => screen.findByRole("group", { name: "Pull requests" });

const openMenu = async (number: number) => {
	fireEvent.click(
		within(await header()).getByRole("button", { name: `More for pull request ${number}` })
	);
	return screen.findByRole("menu");
};

describe("PR chips", () => {
	it("show the agent's PR in its header, and open it beside the conversation", async () => {
		const { ui } = show("agent-usage-popover");
		const chip = within(await header()).getByRole("button", {
			name: "Pull request 142, Usage meter popover, open, checks failing",
		});
		expect(chip).toHaveTextContent("#142");
		expect(chip).toHaveAttribute("title", "Usage meter popover");
		fireEvent.click(chip);
		expect(ui.getState().shownPull).toEqual({
			key: { repo: MOCK_REPO, number: 142 },
			device: expect.any(String),
		});
	});

	it("scroll sideways in their own strip in the header, where a press that scrolled it opens nothing", async () => {
		const { ui } = show("agent-release");
		const group = await header();
		const strip = group.closest<HTMLElement>("[data-chip-strip]")!;
		expect(within(strip).getAllByRole("button", { name: /^Pull request/ })).toHaveLength(2);
		// It grows from about a chip's width up to all of its chips' width,
		// shrinks before the rest of the header, and leaves room for focus rings.
		expect(strip).toHaveClass("grow", "basis-28", "max-w-max", "min-w-0", "overflow-x-auto");
		expect(strip).toHaveClass("-m-1");

		const chip = within(group).getByRole("button", { name: /^Pull request 145,/ });
		fireEvent.pointerDown(chip);
		strip.scrollLeft = 40;
		fireEvent.click(chip);
		expect(ui.getState().shownPull).toBeNull();
		fireEvent.click(chip);
		expect(ui.getState().shownPull).toEqual({
			key: { repo: MOCK_REPO, number: 145 },
			device: expect.any(String),
		});
	});

	it("show small in the sidebar rows, and nowhere for a dismissed link", async () => {
		show("agent-usage-popover");
		await header();
		const small = screen.getByText(", Pull request 143, Sidebar sort order", {
			exact: false,
		}).parentElement!;
		expect(small).toHaveTextContent("#143");
		expect(small).toHaveAttribute("title", "Sidebar sort order");
		expect(screen.queryByText(/Pull request 144/)).not.toBeInTheDocument();
	});

	it("fold into one chip in a sidebar row with two or more, which lists them on hover", async () => {
		const { ui } = show("agent-usage-popover");
		await header();
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const chip = within(nav).getByText(", 2 pull requests: Pull request 141", {
			exact: false,
		}).parentElement!;
		expect(chip).toHaveTextContent("2 PRs");
		expect(chip).toHaveAttribute(
			"title",
			"#141 Show the week under the 5-hour bar\n#145 Release 0.3.0"
		);
		expect(chip.closest("[data-chip-strip]")).toBeNull();
		expect(within(nav).queryByText(/^#141$/)).toBeNull();
		fireEvent.click(chip);
		expect(ui.getState().selectedAgent).toBe("agent-release");
	});

	it("fold past the first into +N in a narrow header, whose menu opens each PR", async () => {
		const { ui } = show("agent-release");
		const group = await header();
		const second = within(group).getByRole("button", { name: /^Pull request 145,/ });
		expect(second.parentElement).toHaveClass("@max-[52rem]/header:hidden");
		const first = within(group).getByRole("button", { name: /^Pull request 141,/ });
		expect(first.parentElement).not.toHaveClass("@max-[52rem]/header:hidden");
		const more = within(group).getByRole("button", { name: "1 more pull request" });
		expect(more).toHaveTextContent("+1");
		expect(more).toHaveClass("hidden", "@max-[52rem]/header:inline-flex");
		expect(group.closest(".\\@container\\/header")).not.toBeNull();

		fireEvent.click(more);
		const menu = await screen.findByRole("menu");
		fireEvent.click(within(menu).getByRole("menuitem", { name: /#145\s*Release 0\.3\.0/ }));
		expect(ui.getState().shownPull).toEqual({
			key: { repo: MOCK_REPO, number: 145 },
			device: expect.any(String),
		});
	});

	it("scroll sideways in their own strip in the sidebar row, which still opens the agent", async () => {
		const { ui } = show("agent-usage-popover");
		await header();
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const chip = within(nav).getByText(", Pull request 145", { exact: false }).parentElement!;
		const strip = chip.closest<HTMLElement>("[data-chip-strip]")!;
		expect(within(strip).getByText("#145")).toBeInTheDocument();
		// It grows with the title up to its chips' width, scrolls with no
		// scrollbar, and the list around it never scrolls sideways.
		expect(strip).toHaveClass("relative", "flex-1", "max-w-max", "min-w-0", "overflow-x-auto");
		expect(strip).toHaveClass("[scrollbar-width:none]", "[&::-webkit-scrollbar]:hidden");
		expect(strip.closest("[data-slot=scroll-area-viewport]")).toHaveClass("overflow-x-hidden!");

		Object.defineProperty(strip, "scrollWidth", { configurable: true, value: 90 });
		Object.defineProperty(strip, "clientWidth", { configurable: true, value: 50 });
		expect(fireEvent.wheel(strip, { deltaY: 30 })).toBe(false);
		expect(strip.scrollLeft).toBe(30);
		fireEvent.scroll(strip);
		expect(strip.style.getPropertyValue("--fade-start")).not.toBe("");

		fireEvent.pointerDown(chip, { pointerType: "touch" });
		strip.scrollLeft = 40;
		fireEvent.click(chip);
		expect(ui.getState().selectedAgent).toBe("agent-usage-popover");
		fireEvent.click(chip);
		expect(ui.getState().selectedAgent).toBe("agent-release-notes");
	});

	it("send an agent MaxHQ holds a message naming the PR from the menu", async () => {
		const { source } = show("agent-release");
		const send = vi.spyOn(source, "sendInput");
		const menu = await openMenu(141);
		expect(within(menu).getByRole("menuitem", { name: "Open on GitHub" })).toHaveAttribute(
			"href",
			`https://github.com/${MOCK_REPO}/pull/141`
		);
		// Its checks pass and it merges cleanly, so it needs no fixes.
		expect(within(menu).queryByRole("menuitem", { name: /^Fix/ })).toBeNull();
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Explain the PR" }));
		await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
		const [agent, input, mode] = send.mock.calls[0]!;
		expect(agent).toBe("agent-release");
		expect(input.text).toContain(`pull request #141 in ${MOCK_REPO}`);
		expect(input.attachments).toEqual([]);
		expect(mode).toBe("queue");
	});

	it("offer no messages for a session in a terminal", async () => {
		show("agent-usage-popover");
		const menu = await openMenu(142);
		expect(within(menu).queryByRole("menuitem", { name: "Explain the PR" })).toBeNull();
		expect(within(menu).getByRole("menuitem", { name: "Unlink" })).toBeInTheDocument();
	});

	it("unlink a PR, then offer to link one from the header menu", async () => {
		show("agent-usage-popover");
		const menu = await openMenu(142);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Unlink" }));
		await waitFor(() =>
			expect(screen.queryByRole("group", { name: "Pull requests" })).not.toBeInTheDocument()
		);
		fireEvent.click(screen.getByRole("button", { name: "More" }));
		expect(
			await screen.findByRole("menuitem", { name: /Link a pull request/ })
		).toBeInTheDocument();
	});

	it("link another PR by its number, and say why the device refused one", async () => {
		show("agent-usage-popover");
		const menu = await openMenu(142);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Link another PR" }));
		const dialog = await screen.findByRole("dialog", { name: "Link a pull request" });
		const field = within(dialog).getByRole("textbox", { name: "Pull request number" });
		const link = within(dialog).getByRole("button", { name: "Link" });
		expect(link).toBeDisabled();

		fireEvent.change(field, { target: { value: "999" } });
		fireEvent.click(link);
		expect(await within(dialog).findByRole("alert")).toHaveTextContent(
			"Couldn't link #999. GitHub has no such PR."
		);

		fireEvent.change(field, { target: { value: "#143" } });
		fireEvent.click(within(dialog).getByRole("button", { name: "Link" }));
		await waitFor(() =>
			expect(
				screen.queryByRole("dialog", { name: "Link a pull request" })
			).not.toBeInTheDocument()
		);
		expect(
			await within(await header()).findByRole("button", { name: /^Pull request 143,/ })
		).toBeInTheDocument();
	});
});
