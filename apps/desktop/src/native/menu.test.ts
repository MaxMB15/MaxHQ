// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Menu } from "@tauri-apps/api/menu";
import { afterEach, expect, it, vi } from "vitest";
import { setLanguageChoice } from "@maxhq/shared/i18n";
import { contextMenu } from "./menu";

afterEach(() => {
	delete (globalThis as { isTauri?: boolean }).isTauri;
	setLanguageChoice("en");
	vi.restoreAllMocks();
});
it("uses the interface language for the native Copy command", () => {
	(globalThis as { isTauri?: boolean }).isTauri = true;
	vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh");
	const create = vi
		.spyOn(Menu, "new")
		.mockResolvedValue({ popup: async () => {}, close: async () => {} } as unknown as Menu);
	setLanguageChoice("de");
	expect(contextMenu([{ kind: "copy", text: "selection" }], { x: 0, y: 0 })).toBe(true);
	expect(create).toHaveBeenCalledWith({ items: [{ item: "Copy", text: "Kopieren" }] });
});
