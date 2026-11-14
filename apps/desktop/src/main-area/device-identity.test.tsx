// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { Entity, LinkClaimState } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { MIN, NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const OK = { status: "ok", data: null } as const;

// The mock source with a second SSH link whose hello says it is devbox.
const withSpare = (source: DataSource, state: LinkClaimState): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const devbox = snapshot.entities.find(
			(e): e is Extract<Entity, { entity: "device_link" }> =>
				e.entity === "device_link" && e.id === "link-devbox"
		);
		if (!devbox) throw new Error("no devbox link");
		const spare: Entity = {
			...devbox,
			id: "link-spare",
			name: "spare",
			device: null,
			route: { type: "ssh", host: "dev@203.0.113.7" },
			routes: [{ route: { type: "ssh", host: "dev@203.0.113.7" }, last_good_here: true }],
			peer: null,
			network: null,
			claim: { device: "dev-devbox", known_by: "link-devbox", state, since: NOW - MIN },
		};
		return { ...snapshot, entities: [...snapshot.entities, spare] };
	},
});

const renderSpare = (state: LinkClaimState) => {
	const source = withSpare(mockSource({ now: () => NOW, tick: false }), state);
	const details = vi.spyOn(source, "linkClaimDetails").mockResolvedValue({
		status: "ok",
		data: {
			link: {
				link: "link-spare",
				name: "spare",
				addresses: ["dev@203.0.113.7"],
				fingerprints: ["ED25519 SHA256:spare"],
				first_seen: NOW - MIN,
			},
			known_by: {
				link: "link-devbox",
				name: "devbox",
				addresses: ["dev@devbox", "dev@devbox.lan"],
				fingerprints: ["ED25519 SHA256:devbox"],
				first_seen: null,
			},
			device_keys: ["ED25519 SHA256:devbox"],
			where_to_change: "Identity, on the device's page in the sidebar",
		},
	});
	const answer = vi.spyOn(source, "answerLinkClaim").mockResolvedValue(OK);
	const again = vi.spyOn(source, "askLinkClaimAgain").mockResolvedValue(OK);
	const ui = createUiStore();
	ui.getState().setMode("devices");
	ui.getState().selectLink("link-spare");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { details, answer, again };
};

const TITLE = "spare says it is devbox, which devbox already knows";

describe("a link whose hello names a device another link knows", () => {
	it("asks with both links' details and the risk, and answers", async () => {
		const { details, answer } = renderSpare("asking");
		const card = await screen.findByRole("region", { name: TITLE });
		expect(card).toHaveTextContent("sign-in tokens and actions meant for devbox");
		await waitFor(() => expect(card).toHaveTextContent("ED25519 SHA256:spare"));
		expect(card).toHaveTextContent("dev@devbox, dev@devbox.lan");
		expect(card).toHaveTextContent(
			"You can change it under Identity, on the device's page in the sidebar."
		);
		expect(details).toHaveBeenCalledWith("link-spare");

		fireEvent.click(within(card).getByRole("button", { name: "Same device" }));
		expect(answer).toHaveBeenCalledWith("link-spare", true);
		const different = within(card).getByRole("button", { name: "Different device" });
		await waitFor(() => expect(different).toBeEnabled());
		fireEvent.click(different);
		expect(answer).toHaveBeenLastCalledWith("link-spare", false);
	});

	it("keeps a note once you said it's a different device, and asks again", async () => {
		const { again } = renderSpare("different");
		const identity = await screen.findByRole("region", { name: "Identity of spare" });
		expect(identity).toHaveTextContent("You said spare isn't devbox");
		expect(screen.queryByRole("region", { name: TITLE })).toBeNull();
		fireEvent.click(within(identity).getByRole("button", { name: "Ask again" }));
		expect(again).toHaveBeenCalledWith("link-spare");
	});
});
