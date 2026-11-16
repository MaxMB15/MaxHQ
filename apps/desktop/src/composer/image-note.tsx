// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The line under the composer's text that says an attached image goes to
// the agent as its file path: its CLI takes no images, or its model can't
// read them.

import type { Account, ModelChoice } from "@maxhq/shared/bindings";
import { imageNote } from "@maxhq/shared/composer/images";
import { useI18n } from "@maxhq/shared/i18n";

import type { Chip } from "./attachments";

export type ImageNoteProps = {
	readonly account: Account | undefined;
	/** The agent's model. */
	readonly model: string | null;
	/** The models its account lists. */
	readonly choices: readonly ModelChoice[];
	/** The files placed in the message. */
	readonly chips: readonly Chip[];
};

/** The note, or nothing while no image is placed or the agent sees them. */
export const ImageNote = ({ account, model, choices, chips }: ImageNoteProps) => {
	useI18n();
	const note = chips.some((chip) => chip.kind === "image")
		? imageNote(account, model, choices)
		: null;
	return note === null ? null : (
		<p className="px-3 pb-1 text-[11.5px] text-muted-foreground">{note}</p>
	);
};
