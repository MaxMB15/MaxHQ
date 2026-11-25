// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The search field in a model picker. A long list, such as the hundreds of
// models an OpenRouter login offers, gets a field that narrows it as you
// type; a short one, as Claude, Codex and Cursor have, shows as it always
// did. A model matches when every word typed shows up in its name or its
// ID, in any case and with or without accents, so "glm 5.3 flash" finds
// "Z.ai: GLM 5.3 Flash" and "glm-5.3" finds `z-ai/glm-5.3-flash`.

/** How many models a picker lists before it gets a search field. */
export const MODEL_SEARCH_AFTER = 12;

/** Whether a picker listing `count` models in all gets a search field. */
export const showsModelSearch = (count: number): boolean => count > MODEL_SEARCH_AFTER;

// Lower case, without accents, so "Modèle" and "modele" are the same.
const fold = (text: string): string => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

/** The words of a query, folded; none for a blank one. */
export const searchWords = (query: string): readonly string[] =>
	fold(query).split(/\s+/).filter(Boolean);

type Searchable = { readonly label: string; readonly id: string };

// A word has no spaces, so it can't run from the name into the ID.
const hasWords = (model: Searchable, words: readonly string[]): boolean => {
	const text = fold(`${model.label}\n${model.id}`);
	return words.every((word) => text.includes(word));
};

/** Whether a model with this name and ID has every word of `query`. A blank query matches all. */
export const matchesModel = (model: Searchable, query: string): boolean =>
	hasWords(model, searchWords(query));

/** The models that match `query`, in their order. A blank query keeps them all. */
export const filterModels = <T extends Searchable>(
	models: readonly T[],
	query: string
): readonly T[] => {
	const words = searchWords(query);
	return words.length === 0 ? models : models.filter((m) => hasWords(m, words));
};
