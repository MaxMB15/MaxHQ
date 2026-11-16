// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the error line says when files don't make it in: the picker or the
// clipboard failing. Each reason gets its own words, and where there's
// another way in, it says so. Files, excerpts and note copies that couldn't
// be readied when you send say so in the shared words of
// @maxhq/shared/composer/copies, which this passes on.

import type { NativeError } from "@maxhq/shared/bindings";
import { NativeFailure } from "@/native";
import { sentence } from "@maxhq/shared/status/format";

import { guessPath, nativeErrorText } from "./native";
import { t as translate } from "@maxhq/shared/i18n";

export {
	attachFailure,
	copyFailure,
	excerptFailure,
	type ExcerptFailure,
} from "@maxhq/shared/composer/copies";

/** How files came in when a native call failed. */
export type Intake = "pick" | "paste";

const INSTEAD: Record<Intake, string> = {
	get pick() {
		return translate("Drop or paste the files instead.");
	},
	get paste() {
		return translate("Drop the files or use the paperclip instead.");
	},
};

const TRIED: Record<Intake, string> = {
	get pick() {
		return translate("Couldn't open the file picker.");
	},
	get paste() {
		return translate("Couldn't read the clipboard.");
	},
};

/** What the error line says when picking or pasting files failed. */
export const intakeFailure = (error: unknown, intake: Intake): string => {
	const reason: NativeError =
		error instanceof NativeFailure
			? error.reason
			: { kind: "failed", message: nativeErrorText(error) };
	switch (reason.kind) {
		case "unsupported":
			return translate("{{sentence}} isn't available on this system. {{value2}}", {
				sentence: sentence(reason.what),
				value2: INSTEAD[intake],
			});
		case "not_offered":
			return translate(
				"MaxHQ won't read {{name}}, since it wasn't dropped, pasted or picked.",
				{ name: guessPath(reason.path).name }
			);
		case "failed": {
			const why = sentence(reason.message);
			return why === "" ? TRIED[intake] : `${TRIED[intake]} ${why}.`;
		}
	}
};
