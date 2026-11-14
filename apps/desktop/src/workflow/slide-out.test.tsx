// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () =>
	render(
		<FixedNow value={NOW}>
			<App source={mockSource({ now: () => NOW, tick: false })} ui={createUiStore()} />
		</FixedNow>
	);

const agents = () => screen.getByRole("navigation", { name: "Agents" });

/** Opens a lead from the sidebar and returns its slide-out. */
const openLead = async (title: string) => {
	fireEvent.click(await within(agents()).findByRole("option", { name: new RegExp(title) }));
	return screen.getByRole("complementary", { name: `Sub-agents of ${title}` });
};

const widthOf = (panel: HTMLElement) => panel.style.width;

describe("the slide-out", () => {
	it("stays narrow with no tabs while the lead has no run", async () => {
		renderApp();
		const panel = await openLead("M1 observe milestone");
		expect(widthOf(panel)).toBe("216px");
		expect(within(panel).queryByRole("tablist")).not.toBeInTheDocument();
		expect(within(panel).getByRole("button", { name: /W4 client crate/ })).toBeInTheDocument();
	});

	it("is 320 px on both tabs for a lead with a run", async () => {
		renderApp();
		const panel = await openLead("Search filters");
		expect(widthOf(panel)).toBe("320px");
		expect(within(panel).getByRole("tab", { name: "Sub-agents" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		expect(within(panel).getByRole("button", { name: /Testing gate/ })).toBeInTheDocument();

		fireEvent.click(within(panel).getByRole("tab", { name: "Workflow" }));
		expect(widthOf(panel)).toBe("320px");
		const nodes = await within(panel).findByRole("list", { name: "Nodes" });
		expect(within(nodes).getByRole("button", { name: /^Research, Done/ })).toBeInTheDocument();
		expect(
			within(panel).queryByRole("button", { name: /Testing gate/ })
		).not.toBeInTheDocument();
	});

	it("opens on the Workflow tab for a lead with a run and no sub-agents", async () => {
		renderApp();
		const panel = await openLead("Offline sync");
		expect(widthOf(panel)).toBe("320px");
		expect(within(panel).getByRole("tab", { name: "Workflow" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		expect(await within(panel).findByRole("list", { name: "Nodes" })).toBeInTheDocument();

		// The tab you pick is the one it opens on next time.
		fireEvent.click(within(panel).getByRole("tab", { name: "Sub-agents" }));
		fireEvent.click(within(panel).getByRole("button", { name: "Close sub-agents" }));
		fireEvent.click(within(agents()).getByRole("option", { name: /Offline sync/ }));
		expect(
			within(screen.getByRole("complementary")).getByRole("tab", { name: "Sub-agents" })
		).toHaveAttribute("aria-selected", "true");
	});

	it("opens the Workflow tab from the header chip, and the lead keeps its last tab", async () => {
		renderApp();
		await openLead("Search filters");
		fireEvent.click(screen.getByRole("button", { name: /^Workflow, Search filters, / }));
		const panel = screen.getByRole("complementary", { name: "Sub-agents of Search filters" });
		expect(within(panel).getByRole("tab", { name: "Workflow" })).toHaveAttribute(
			"aria-selected",
			"true"
		);

		fireEvent.click(within(panel).getByRole("button", { name: "Close sub-agents" }));
		expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
		fireEvent.click(within(agents()).getByRole("option", { name: /Search filters/ }));
		expect(
			within(screen.getByRole("complementary")).getByRole("tab", { name: "Workflow" })
		).toHaveAttribute("aria-selected", "true");
	});

	it("puts the latest run's chip on the lead's row, which opens its Workflow tab", async () => {
		renderApp();
		const row = await within(agents()).findByRole("option", { name: /Search filters/ });
		expect(row).toHaveTextContent(/pipeline \d+\/\d+/);
		expect(row).toHaveAccessibleName(/Search filters, \d+ of \d+ done/);
		fireEvent.click(within(row).getByText(/^pipeline /));
		const panel = screen.getByRole("complementary", { name: "Sub-agents of Search filters" });
		expect(within(panel).getByRole("tab", { name: "Workflow" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
	});

	describe("from a sub-agent card", () => {
		// jsdom lays nothing out, and the timeline draws no rows into a window
		// of no height. It has no scrollIntoView either.
		const scrolled = vi.fn<(options?: ScrollIntoViewOptions | boolean) => void>();
		beforeAll(() => {
			vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(2000);
			vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(800);
			Element.prototype.scrollIntoView = scrolled;
		});
		afterAll(() => {
			vi.restoreAllMocks();
			delete (Element.prototype as Partial<Element>).scrollIntoView;
		});

		const cardOf = async (description: string) => {
			const text = await within(screen.getByRole("main")).findByText(description);
			return text.closest<HTMLElement>(".rounded-lg")!;
		};
		const showButton = (card: HTMLElement) =>
			within(card).getByRole("button", { name: /^Show(n)? in panel$/ });

		it("points out that sub-agent's row, and only its card reads as shown", async () => {
			renderApp();
			const panel = await openLead("M1 observe milestone");
			const w4 = await cardOf("W4 client crate");
			const w2 = await cardOf("W2 Claude observer");
			expect(showButton(w4)).toHaveTextContent("Show in panel");

			scrolled.mockClear();
			fireEvent.click(showButton(w4));
			expect(showButton(w4)).toHaveTextContent("Shown in panel");
			expect(showButton(w4)).toHaveAttribute("aria-pressed", "true");
			expect(showButton(w2)).toHaveTextContent("Show in panel");
			expect(showButton(w2)).toHaveAttribute("aria-pressed", "false");
			const row = within(panel).getByRole("button", { name: /W4 client crate/ });
			expect(row).toHaveAttribute("data-spotted", "true");
			expect(scrolled).toHaveBeenCalledTimes(1);
			expect(scrolled.mock.contexts[0]).toBe(row);
			expect(
				within(panel).getByRole("button", { name: /W2 Claude observer/ })
			).not.toHaveAttribute("data-spotted");

			// Another card moves the ring to its own row.
			fireEvent.click(showButton(w2));
			expect(showButton(w2)).toHaveTextContent("Shown in panel");
			expect(showButton(w4)).toHaveTextContent("Show in panel");
			expect(row).not.toHaveAttribute("data-spotted");
			expect(scrolled.mock.contexts.at(-1)).toBe(
				within(panel).getByRole("button", { name: /W2 Claude observer/ })
			);

			// The shown card closes the panel.
			fireEvent.click(showButton(w2));
			expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
			expect(showButton(w2)).toHaveTextContent("Show in panel");
		});
	});

	it("opens a run with no lead from the project's Workflows entry, in run mode", async () => {
		renderApp();
		const entry = await screen.findByRole("group", { name: "Workflows in example-api" });
		fireEvent.click(within(entry).getByRole("button", { name: /Webhook retries/ }));

		const panel = screen.getByRole("complementary", { name: "Workflow run Webhook retries" });
		expect(widthOf(panel)).toBe("320px");
		expect(within(panel).queryByRole("tablist")).not.toBeInTheDocument();
		expect(await within(panel).findByRole("list", { name: "Nodes" })).toBeInTheDocument();
		expect(within(entry).getByRole("button", { name: /Webhook retries/ })).toHaveAttribute(
			"aria-current",
			"true"
		);

		fireEvent.click(within(panel).getByRole("button", { name: "Close workflow" }));
		expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
	});
});
