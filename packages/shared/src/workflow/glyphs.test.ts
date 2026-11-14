// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, expect, it } from "vitest";
import { setLanguageChoice, t } from "../i18n";
import { GLYPHS, glyphOf } from "./glyphs";

afterEach(() => setLanguageChoice("en"));
it("updates labels of already created glyphs when the language changes", () => {
	const queued = GLYPHS.queued;
	expect(queued.label).toBe("Queued");
	setLanguageChoice("de");
	expect(t("Queued")).not.toBe("Queued");
	expect(queued.label).toBe(t("Queued"));
	expect(glyphOf("queued").label).toBe(t("Queued"));
});
