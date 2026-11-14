// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { MIN, NOW } from "@maxhq/shared/test/build";
import { createUiStore, type SidebarMode } from "@maxhq/shared/ui/store";

const TITLE = "Let this program control devbox?";
const RISK = "A program connected to maxd on devbox without MaxHQ's app key.";
const DONE = { status: "ok", data: { status: "done" } } as const;

// The mock source with a card devbox raised about a program.
const withCard = (source: DataSource): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const card: Entity = {
			entity: "admission",
			id: "adm-1",
			device: "dev-devbox",
			raised_at: NOW - MIN,
			expires_at: NOW + 9 * MIN,
			question: {
				id: "adm-1",
				header: null,
				text: `${TITLE}\n\n${RISK}`,
				options: [
					{ label: "Don't allow", description: null },
					{ label: "Allow until it disconnects", description: null },
				],
				multi: false,
				other: false,
				secret: false,
				consent: {
					title: TITLE,
					risk: RISK,
					changes: [],
					facts: [
						{ label: "Device", value: "devbox" },
						{ label: "Says it is", value: "MaxHQ 0.9.0" },
						{ label: "How it reached maxd", value: "SSH, through maxd proxy" },
					],
					rows: [],
					remember: null,
				},
			},
		};
		return { ...snapshot, entities: [...snapshot.entities, card] };
	},
});

const renderCard = (mode: SidebarMode) => {
	const source = withCard(mockSource({ now: () => NOW, tick: false }));
	const answer = vi.spyOn(source, "answerAdmission").mockResolvedValue(DONE);
	const ui = createUiStore();
	ui.getState().setMode(mode);
	if (mode === "devices") ui.getState().selectLink("link-devbox");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { answer, ui };
};

describe("a card a device raised about a program", () => {
	it("shows on the device's page with Don't allow first, and answers", async () => {
		const { answer } = renderCard("devices");
		const card = await screen.findByRole("region", { name: TITLE });
		expect(card).toHaveTextContent(RISK);
		expect(card).toHaveTextContent("SSH, through maxd proxy");
		expect(card).toHaveTextContent("Nothing is kept. If it connects again, it asks again.");
		expect(within(card).queryByRole("checkbox")).toBeNull();
		const buttons = within(card).getAllByRole("button");
		expect(buttons.map((b) => b.textContent)).toEqual([
			"Don't allow",
			"Allow until it disconnects",
		]);

		fireEvent.click(within(card).getByRole("button", { name: "Don't allow" }));
		expect(answer).toHaveBeenCalledWith("adm-1", false);
		const allow = within(card).getByRole("button", { name: "Allow until it disconnects" });
		await waitFor(() => expect(allow).toBeEnabled());
		fireEvent.click(allow);
		expect(answer).toHaveBeenLastCalledWith("adm-1", true);
	});

	it("waits in the sidebar, and a click opens the device's page", async () => {
		const { ui } = renderCard("projects");
		const strip = await screen.findByRole("region", { name: "1 program asking to get in" });
		fireEvent.click(within(strip).getByText(TITLE));
		expect(ui.getState().selectedLink).toBe("link-devbox");
		expect(ui.getState().mode).toBe("devices");
		await screen.findByRole("region", { name: TITLE });
	});
});
