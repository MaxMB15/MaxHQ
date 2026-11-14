// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the error line says when files don't make it in: the picker or the
// clipboard failing, or files and excerpts of other devices' agents that
// couldn't be readied when you send. Each reason gets its own words, and
// where there's another way in, it says so.

import type { NativeError } from "@maxhq/shared/bindings";
import { NativeFailure } from "@/native";
import { sentence } from "@maxhq/shared/status/format";

import type { Chip } from "./attachments";
import { guessPath, nativeErrorText } from "./native";
import { t as translate } from "@maxhq/shared/i18n";

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

// What a chip says when staging failed with no reason. Saying it twice
// adds nothing, so the error line leaves it out.
const NO_REASON = "Couldn't get it ready";

/** The reason every one of them failed for, as a sentence to add, or nothing when they differ. */
const sharedReason = (errors: readonly (string | null)[]): string => {
	const reasons = new Set(errors.map((error) => error ?? NO_REASON));
	const [only] = reasons;
	return reasons.size === 1 && only !== undefined && only !== NO_REASON && sentence(only) !== ""
		? ` ${sentence(only)}.`
		: "";
};

/** What the error line says when a message waits on files that couldn't be readied. */
export const attachFailure = (failed: readonly Pick<Chip, "name" | "error">[]): string => {
	const why = sharedReason(failed.map((chip) => chip.error));
	const [first] = failed;
	return failed.length === 1 && first
		? translate("Couldn't attach {{name}}.{{why}} Remove it, then send again.", {
				name: first.name,
				why: why,
			})
		: translate("Couldn't attach {{length}} files.{{why}} Remove them, then send again.", {
				length: failed.length,
				why: why,
			});
};

/** An agent on another device whose excerpt couldn't be taken or readied, and why. */
export type ExcerptFailure = { readonly title: string; readonly error: string | null };

/**
 * What the error line says when a message waits on excerpts of agents on
 * other devices that couldn't be taken or readied. Sending again takes
 * them again.
 */
export const excerptFailure = (failed: readonly ExcerptFailure[]): string => {
	const why = sharedReason(failed.map((each) => each.error));
	const [first] = failed;
	return failed.length === 1 && first
		? translate(
				"Couldn't get an excerpt of {{title}}.{{why}} Send again, or remove the agent.",
				{ title: first.title, why: why }
			)
		: translate(
				"Couldn't get excerpts of {{length}} agents.{{why}} Send again, or remove them.",
				{ length: failed.length, why: why }
			);
};

/**
 * What the error line says when a message waits on copies of notes from
 * other projects that couldn't be read or readied. Sending again reads
 * them again.
 */
export const copyFailure = (failed: readonly ExcerptFailure[]): string => {
	const why = sharedReason(failed.map((each) => each.error));
	const [first] = failed;
	return failed.length === 1 && first
		? translate("Couldn't copy the note {{title}}.{{why}} Send again, or remove the note.", {
				title: first.title,
				why: why,
			})
		: translate("Couldn't copy {{length}} notes.{{why}} Send again, or remove them.", {
				length: failed.length,
				why: why,
			});
};
