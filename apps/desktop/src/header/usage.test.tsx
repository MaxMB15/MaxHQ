// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { mockSource } from "@maxhq/shared/data/mock-source";
import { usagePlans } from "@maxhq/shared/status/usage-text";
import { MIN, NOW } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { AccountSection, Bar } from "./usage";

const plans = async () => {
	const view = applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());
	return { view, plans: usagePlans(view) };
};

/** The cells of the "By model" row whose name cell starts with `label`. */
const modelRow = (section: HTMLElement, label: string) => {
	const name = within(section).getAllByTitle(label)[0]!.parentElement!;
	const cells = [...name.parentElement!.children] as HTMLElement[];
	return cells.map((cell) => cell.textContent);
};

describe("AccountSection", () => {
	it("lists each model's window with its percent and reset under the account", async () => {
		const { view, plans: list } = await plans();
		const codex = list.find((p) => p.account.vendor === "codex")!;
		render(<AccountSection plan={codex} view={view} now={NOW} first />);
		const section = screen.getByRole("region", { name: "Codex Pro" });
		expect(within(section).getByText("By model")).toBeTruthy();
		const rows = within(section).getAllByTitle("Codex Mini");
		expect(rows).toHaveLength(2);
		// Codex Mini's 5 hours run out before the account's do, so it shows
		// the account's later reset.
		expect(modelRow(section, "Codex Mini")).toEqual([
			"Codex Mini5h",
			"",
			"100%",
			"resets in 2h, 17:10",
		]);
		const reset = within(section).getAllByText("resets in 2h, 17:10");
		expect(reset.every((cell) => cell.title.includes("17:10"))).toBe(true);
	});

	it("folds a long list to the three fullest with a Show all switch", async () => {
		const { view, plans: list } = await plans();
		const claude = list.find((p) => p.account.vendor === "claude")!;
		const scope = (s: string, percent: number) => ({
			scope: s,
			window_minutes: 10080,
			used_percent: percent,
			resets_at: NOW + 3 * 24 * 60 * MIN,
		});
		const many = {
			...claude,
			account: {
				...claude.account,
				usage: {
					...claude.account.usage!,
					windows: [
						scope("opus", 10),
						scope("sonnet", 80),
						scope("haiku", 30),
						scope("fable", 60),
						scope("codex_mini", 5),
					],
				},
			},
		};
		render(<AccountSection plan={many} view={view} now={NOW} first />);
		const names = () => screen.queryAllByTitle(/^(Opus|Sonnet|Haiku|Fable|Codex Mini)$/);
		expect(names().map((n) => n.textContent)).toEqual(["Sonnet", "Haiku", "Fable"]);
		fireEvent.click(screen.getByRole("button", { name: "Show all" }));
		expect(names()).toHaveLength(5);
		fireEvent.click(screen.getByRole("button", { name: "Show less" }));
		expect(names()).toHaveLength(3);
	});

	it("leaves the models list out when the account has none", async () => {
		const { view, plans: list } = await plans();
		const plan = list.find((p) => p.account.vendor === "claude")!;
		render(
			<AccountSection
				plan={{ ...plan, account: { ...plan.account, usage: null } }}
				view={view}
				now={NOW}
				first
			/>
		);
		expect(screen.queryByText("By model")).toBeNull();
	});
});

const layers = (container: HTMLElement) => ({
	main: container.querySelector<HTMLElement>('[data-layer="main"]'),
	under: container.querySelector<HTMLElement>('[data-layer="under"]'),
});

describe("Bar", () => {
	it("draws the under figure beneath the main fill, both within the track", () => {
		const { container } = render(<Bar percent={40} under={130} full={false} className="h-1" />);
		const { main, under } = layers(container);
		expect(main?.style.width).toBe("40%");
		expect(under?.style.width).toBe("100%");
		expect(under?.compareDocumentPosition(main!)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
	});

	it("draws one fill without an under figure", () => {
		const { container } = render(<Bar percent={-5} full={false} className="h-1" />);
		const { main, under } = layers(container);
		expect(main?.style.width).toBe("0%");
		expect(under).toBeNull();
	});

	it("keeps the busy color on both layers when full", () => {
		const { container } = render(<Bar percent={100} under={70} full className="h-1" />);
		const { main, under } = layers(container);
		expect(main?.className).toContain("bg-status-busy");
		expect(under?.className).toContain("bg-status-busy");
	});
});
