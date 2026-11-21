// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import { type AccountId, type ActionOutcome, RESET_CREDITS_SINCE } from "@maxhq/shared/bindings";
import { mockSource } from "@maxhq/shared/data/mock-source";
import { CODEX, MAC } from "@maxhq/shared/data/mock-view";
import type { Result } from "@maxhq/shared/data/request-source";
import { usagePlans } from "@maxhq/shared/status/usage-text";
import { MIN, NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
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

/** The sample view with the Codex Pro login's resets banked and the Mac's maxd at `protocol`. */
const banked = async (count: number, protocol: number = RESET_CREDITS_SINCE) => {
	const snapshot = await mockSource({ now: () => NOW, tick: false }).snapshot();
	const view = applySnapshot({
		...snapshot,
		entities: snapshot.entities.map((e) => {
			if (e.entity === "account" && e.id === CODEX && e.usage) {
				return { ...e, usage: { ...e.usage, reset_credits: count } };
			}
			if (e.entity === "device_link" && e.device === MAC) return { ...e, protocol };
			return e;
		}),
	});
	const account = view.tables.account.get(CODEX)!;
	const plan = usagePlans(view).find((p) => p.account.vendor === "codex")!;
	return { view, plan: { ...plan, account } };
};

type Spend = (account: AccountId) => Promise<Result<ActionOutcome>>;

/** The Codex section in a window whose resets go to `useResetCredit`. */
const renderBanked = async (useResetCredit: Spend, count = 2, protocol?: number) => {
	const { view, plan } = await banked(count, protocol);
	const source = { ...mockSource({ now: () => NOW, tick: false }), useResetCredit };
	render(
		<ServicesProvider source={source} ui={createUiStore()}>
			<AccountSection plan={plan} view={view} now={NOW} first />
			<ConfirmHost />
		</ServicesProvider>
	);
	return screen.getByRole("region", { name: "Codex Pro" });
};

const RESET: Result<ActionOutcome> = {
	status: "ok",
	data: { status: "reset_credit", outcome: "reset" },
};

describe("banked resets", () => {
	it("shows how many are banked, and Cancel spends nothing", async () => {
		const spend = vi.fn<Spend>(async () => RESET);
		const section = await renderBanked(spend);
		expect(within(section).getByText("2 resets banked")).toBeInTheDocument();

		fireEvent.click(within(section).getByRole("button", { name: "Use reset" }));
		const dialog = await screen.findByRole("alertdialog");
		expect(within(dialog).getByText("Use a Codex reset?")).toBeInTheDocument();
		expect(
			within(dialog).getByText(
				"This spends 1 of your 2 banked resets and resets this account's Codex limits now. It can't be undone."
			)
		).toBeInTheDocument();
		const cancel = within(dialog).getByRole("button", { name: "Cancel" });
		await waitFor(() => expect(cancel).toHaveFocus());
		fireEvent.click(cancel);
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(spend).not.toHaveBeenCalled();
		expect(within(section).queryByRole("status")).toBeNull();
	});

	it("spends one once you confirm and shows what came of it", async () => {
		const spend = vi.fn<Spend>(async () => RESET);
		const section = await renderBanked(spend);

		fireEvent.click(within(section).getByRole("button", { name: "Use reset" }));
		const dialog = await screen.findByRole("alertdialog");
		fireEvent.click(within(dialog).getByRole("button", { name: "Use reset" }));

		expect(
			await within(section).findByText(
				"Reset used. This account's Codex limits start over now."
			)
		).toBeInTheDocument();
		expect(spend).toHaveBeenCalledTimes(1);
		expect(spend).toHaveBeenCalledWith(CODEX);
	});

	it("can't be pressed again while a spend is out", async () => {
		let answer: (result: Result<ActionOutcome>) => void = () => {};
		const spend = vi.fn<Spend>(
			() =>
				new Promise((resolve) => {
					answer = resolve;
				})
		);
		const section = await renderBanked(spend, 1);

		fireEvent.click(within(section).getByRole("button", { name: "Use reset" }));
		const dialog = await screen.findByRole("alertdialog");
		expect(
			within(dialog).getByText(
				"This spends your only banked reset and resets this account's Codex limits now. It can't be undone."
			)
		).toBeInTheDocument();
		fireEvent.click(within(dialog).getByRole("button", { name: "Use reset" }));

		const running = await within(section).findByRole("button", { name: "Using a reset…" });
		expect(running).toBeDisabled();
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		// A second press asks nothing and sends nothing.
		fireEvent.click(running);
		expect(screen.queryByRole("alertdialog")).toBeNull();

		answer({ status: "ok", data: { status: "reset_credit", outcome: "nothing_to_reset" } });
		expect(
			await within(section).findByText(
				"No limit needs a reset right now, so nothing was spent."
			)
		).toBeInTheDocument();
		expect(spend).toHaveBeenCalledTimes(1);
	});

	it("leaves Use reset out where the device's maxd can't spend one", async () => {
		const section = await renderBanked(vi.fn<Spend>(), 2, RESET_CREDITS_SINCE - 1);
		expect(within(section).getByText("2 resets banked")).toBeInTheDocument();
		expect(within(section).queryByRole("button", { name: "Use reset" })).toBeNull();
	});

	it("says nothing about resets when none are banked", async () => {
		const section = await renderBanked(vi.fn<Spend>(), 0);
		expect(within(section).queryByText(/banked/)).toBeNull();
		expect(within(section).queryByRole("button", { name: "Use reset" })).toBeNull();
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
