// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, expect, it } from "vitest";
import { setLanguageChoice } from "../i18n";
import { timelineAsk } from "./timeline";

afterEach(() => setLanguageChoice("en"));

it("recognizes an unreadable timeline's wire error in every interface language", async () => {
	for (const language of ["en", "nl", "de", "fr", "es"] as const) {
		setLanguageChoice(language);
		const ask = timelineAsk(
			{
				findInTimeline: async () => {
					throw new Error("can't read that agent's timeline");
				},
			},
			"a1"
		);
		await expect(ask("query", { type: "index", index: 0 })).resolves.toEqual({
			total: 0,
			active: 0,
			found: null,
			near: [],
		});
	}
});
