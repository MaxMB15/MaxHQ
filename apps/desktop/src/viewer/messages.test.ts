// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { isOpenFile } from "./messages";

const open = (theme: unknown) => ({
	type: "maxhq-viewer:open",
	name: "meter-stand.glb",
	extension: "glb",
	bytes: new ArrayBuffer(4),
	theme,
});

describe("a file to open", () => {
	it.each(["light", "dark"])("takes the %s theme", (theme) => {
		expect(isOpenFile(open(theme))).toBe(true);
	});

	it.each([undefined, "system", "blue"])("refuses a theme of %s", (theme) => {
		expect(isOpenFile(open(theme))).toBe(false);
	});
});
