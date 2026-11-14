// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether you confirmed Auto mode on this phone. It stays in a file in the
// app's documents, so the phone asks once per install. A message to an
// agent in an Auto you didn't pick asks too, with Ask first instead as the
// other answer.

import { File, Paths } from "expo-file-system";

import {
	AUTO_DEFAULT_QUESTION,
	type AutoChoice,
	keptConsent,
} from "@maxhq/shared/settings/auto-mode";

import { answer } from "../../ui/choose";

const kept = () => new File(Paths.document, "auto-mode.txt");

export const autoConsent = keptConsent({
	read: () => {
		const file = kept();
		return file.exists ? file.textSync() : null;
	},
	write: (value) => kept().write(value),
});

/** Asks `AUTO_DEFAULT_QUESTION` in the system's alert. */
export const askAutoDefault = async (): Promise<AutoChoice | null> => {
	const pressed = await answer(AUTO_DEFAULT_QUESTION);
	if (pressed === null) return null;
	return pressed === "action" ? "auto" : "ask";
};
