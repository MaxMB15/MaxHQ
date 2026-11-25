// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	filterModels,
	MODEL_SEARCH_AFTER,
	matchesModel,
	searchWords,
	showsModelSearch,
} from "./model-search";

const models = [
	{ id: "anthropic/z-ai/glm-5.3-flash[1m]", label: "Z.ai: GLM 5.3 Flash" },
	{ id: "anthropic/z-ai/glm-5.3", label: "Z.ai: GLM 5.3" },
	{ id: "anthropic/openai/gpt-5.5", label: "OpenAI: GPT-5.5" },
	{ id: "anthropic/google/gemini-3-flash", label: "Google: Gemini 3 Flash" },
	{ id: "", label: "Modèle par défaut" },
];

const labels = (query: string) => filterModels(models, query).map((m) => m.label);

describe("the model search", () => {
	it("finds a model by every word of its name, in any case and order", () => {
		expect(labels("glm 5.3 flash")).toEqual(["Z.ai: GLM 5.3 Flash"]);
		expect(labels("FLASH glm")).toEqual(["Z.ai: GLM 5.3 Flash"]);
		expect(labels("flash")).toEqual(["Z.ai: GLM 5.3 Flash", "Google: Gemini 3 Flash"]);
	});

	it("finds a model by its ID, and by name and ID words together", () => {
		expect(labels("glm-5.3-flash")).toEqual(["Z.ai: GLM 5.3 Flash"]);
		expect(labels("[1m]")).toEqual(["Z.ai: GLM 5.3 Flash"]);
		expect(labels("z-ai flash")).toEqual(["Z.ai: GLM 5.3 Flash"]);
		expect(labels("openai/gpt")).toEqual(["OpenAI: GPT-5.5"]);
	});

	it("keeps every model for a blank query, in their order", () => {
		expect(filterModels(models, "")).toBe(models);
		expect(filterModels(models, "   \t ")).toBe(models);
		expect(matchesModel({ id: "x", label: "X" }, "")).toBe(true);
	});

	it("needs every word, and finds nothing when one is missing", () => {
		expect(labels("glm opus")).toEqual([]);
		expect(labels("sonnet")).toEqual([]);
	});

	it("ignores accents either way", () => {
		expect(labels("modele defaut")).toEqual(["Modèle par défaut"]);
		expect(labels("MODÈLE")).toEqual(["Modèle par défaut"]);
		expect(searchWords("  Modèle   PAR\tdéfaut ")).toEqual(["modele", "par", "defaut"]);
	});

	it("doesn't run a word from the name into the ID", () => {
		// The name ends in "5.3" and the ID starts with "anthropic".
		expect(matchesModel(models[1]!, "5.3anthropic")).toBe(false);
	});
});

describe("when a picker gets a search field", () => {
	it("is past twelve models in all", () => {
		expect(MODEL_SEARCH_AFTER).toBe(12);
		expect(showsModelSearch(0)).toBe(false);
		expect(showsModelSearch(3)).toBe(false);
		expect(showsModelSearch(12)).toBe(false);
		expect(showsModelSearch(13)).toBe(true);
		expect(showsModelSearch(376)).toBe(true);
	});
});
