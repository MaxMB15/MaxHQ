// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ModelChoice } from "../bindings";
import { modelWithoutImages } from "../agent-view/controls";
import { account } from "../test/build";

import { imageNote } from "./images";

const CURSOR: ModelChoice[] = [
	{ id: "default", label: "Auto", default: true },
	{ id: "glm-5p3-flash", label: "GLM 5.3 Flash", default: false, no_images: true },
	{ id: "composer-2.5", label: "Composer 2.5", default: false },
];

describe("modelWithoutImages", () => {
	it("names a model that can't read images", () => {
		expect(modelWithoutImages("glm-5p3-flash", CURSOR)).toBe("GLM 5.3 Flash");
	});

	it("gives null for a model that reads them, or one the list doesn't know", () => {
		expect(modelWithoutImages("composer-2.5", CURSOR)).toBeNull();
		expect(modelWithoutImages(null, CURSOR)).toBeNull();
		expect(modelWithoutImages("glm-6", CURSOR)).toBeNull();
		expect(modelWithoutImages("glm-5p3-flash", [])).toBeNull();
	});

	it("reads the account's default when the agent names no model", () => {
		const glm = CURSOR.map((choice) => ({ ...choice, default: choice.id === "glm-5p3-flash" }));
		expect(modelWithoutImages(null, glm)).toBe("GLM 5.3 Flash");
	});
});

describe("imageNote", () => {
	const cursor = account({ id: "a-cursor", vendor: "cursor", label: "Cursor" });

	it("says a model that can't read images gets the file path", () => {
		expect(imageNote(cursor, "glm-5p3-flash", CURSOR)).toBe(
			"GLM 5.3 Flash can't see images. The agent gets the file path instead."
		);
		expect(imageNote(cursor, "composer-2.5", CURSOR)).toBeNull();
		expect(imageNote(undefined, "composer-2.5", CURSOR)).toBeNull();
	});

	it("says a local ACP agent that takes no images gets the file path", () => {
		const gemini = account({
			id: "a-acp",
			vendor: "acp",
			label: "Gemini",
			acp: { images: false },
		});
		expect(imageNote(gemini, null, [])).toBe(
			"Gemini doesn't take images, so they go as file paths in the message."
		);
		const sees = account({
			id: "a-acp",
			vendor: "acp",
			label: "Gemini",
			acp: { images: true },
		});
		expect(imageNote(sees, null, [])).toBeNull();
	});
});
