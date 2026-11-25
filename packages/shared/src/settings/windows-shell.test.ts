// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { WINDOWS_SHELL_SINCE } from "../bindings";
import { device, entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";

import { picksWindowsShell, WINDOWS_SHELL_OPTIONS, windowsShellOf } from "./windows-shell";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [
			device({ id: "dev-pc", os: "windows" }),
			device({ id: "dev-old", os: "windows" }),
			device({ id: "dev-box", os: "linux" }),
			device({ id: "dev-mac", os: "macos" }),
		],
		links: [
			link({ id: "pc", protocol: WINDOWS_SHELL_SINCE }),
			link({ id: "old", protocol: WINDOWS_SHELL_SINCE - 1 }),
			link({ id: "box", protocol: WINDOWS_SHELL_SINCE }),
			link({ id: "mac", protocol: WINDOWS_SHELL_SINCE }),
		],
	}),
});

describe("picksWindowsShell", () => {
	it("offers the choice on a Windows device whose maxd keeps it", () => {
		expect(picksWindowsShell(view, "dev-pc")).toBe(true);
	});

	it("leaves it out on an older maxd and on other systems", () => {
		expect(picksWindowsShell(view, "dev-old")).toBe(false);
		expect(picksWindowsShell(view, "dev-box")).toBe(false);
		expect(picksWindowsShell(view, "dev-mac")).toBe(false);
		expect(picksWindowsShell(view, "dev-missing")).toBe(false);
	});
});

describe("WINDOWS_SHELL_OPTIONS", () => {
	it("lists Automatic first, then each shell", () => {
		expect(WINDOWS_SHELL_OPTIONS.map((option) => option.value)).toEqual([
			"auto",
			"git_bash",
			"pwsh",
			"power_shell",
		]);
		expect(WINDOWS_SHELL_OPTIONS.map((option) => option.label)).toEqual([
			"Automatic",
			"Git Bash",
			"PowerShell 7",
			"Windows PowerShell 5.1",
		]);
	});

	it("reads an unknown value as Automatic", () => {
		expect(windowsShellOf("pwsh")).toBe("pwsh");
		expect(windowsShellOf("cmd")).toBe("auto");
	});
});
