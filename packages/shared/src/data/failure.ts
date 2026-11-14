// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a change came back with, in words: why it didn't go through, or
// nothing when it did. The desktop's settings and the dispatcher on every
// client say it the same way. A reply to a change with a secret in it goes
// through `scrub` before it shows.

import { t } from "../i18n";

import type { ActionResult, CommandResult } from "./source";

/** Why the change didn't go through, or null when it did. */
export const failureOf = (result: ActionResult | CommandResult): string | null => {
	if (result.status === "error") {
		const { error } = result;
		switch (error.kind) {
			case "not_connected":
				return t("The device isn't connected, so nothing changed");
			case "not_found":
				return t("Couldn't find {{error_what}}", { error_what: error.what });
			case "failed":
				return error.message;
		}
	}
	return result.data?.status === "refused" ? result.data.reason : null;
};

/**
 * The text with the secret replaced by `<secret>`. A device that echoes what
 * it was sent in an error would otherwise put the secret on the screen.
 */
export const scrub = (text: string, secret: string): string =>
	secret ? text.split(secret).join("<secret>") : text;
