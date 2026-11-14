// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { PeerLink } from "../bindings";

import { linkAction, linkSub } from "./device-links";

const link = (state: PeerLink["state"], note: string | null = null): PeerLink => ({
	peer: "dev-devbox",
	name: "devbox",
	state,
	note,
});

describe("device links", () => {
	it("says where a link stands, or why it can't link yet", () => {
		expect(linkSub(link("linked"))).toBe("Linked");
		expect(linkSub(link("waiting"))).toBe("Waiting for the device");
		expect(linkSub(link("off"))).toBe("Off");
		expect(linkSub(link("waiting", "Set it up again"))).toBe("Set it up again");
	});

	it("offers Remove on a link that's on and Allow on one that's off", () => {
		expect(linkAction(link("linked"))).toEqual({ label: "Remove", on: false });
		expect(linkAction(link("waiting"))).toEqual({ label: "Remove", on: false });
		expect(linkAction(link("off"))).toEqual({ label: "Allow", on: true });
	});
});
