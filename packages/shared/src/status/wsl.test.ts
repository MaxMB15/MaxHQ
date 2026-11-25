// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { link } from "../test/build";
import { wslCommand, wslLabel, wslMachine, type WslRoute } from "./wsl";

const here: WslRoute = { type: "wsl", distro: "Ubuntu", via: null };
const there: WslRoute = { type: "wsl", distro: "Ubuntu", via: "dev@pc" };

describe("WSL routes", () => {
	it("name the Windows machine by the device its SSH destination reaches", () => {
		const pc = link({
			id: "pc",
			name: "Desk PC",
			route: { type: "ssh", host: "dev@pc-lan" },
			routes: [
				{ route: { type: "ssh", host: "dev@pc-lan" }, last_good_here: true },
				{ route: { type: "ssh", host: "dev@pc" }, last_good_here: false },
			],
		});
		expect(wslMachine([link({ id: "box" }), pc], there)).toBe("Desk PC");
		expect(wslMachine([link({ id: "box" })], there)).toBe("dev@pc");
		expect(wslMachine([pc], here)).toBeNull();
	});

	it("say the distro and the machine it's on", () => {
		expect(wslLabel(here)).toBe("WSL Ubuntu on this PC");
		expect(wslLabel(there)).toBe("WSL Ubuntu on dev@pc");
		expect(wslLabel(there, "Desk PC")).toBe("WSL Ubuntu on Desk PC");
	});

	it("open a shell through wsl.exe, over SSH for another PC", () => {
		expect(wslCommand(here)).toBe("wsl.exe -d Ubuntu");
		expect(wslCommand(there)).toBe("ssh -t dev@pc wsl.exe -d Ubuntu");
	});
});
