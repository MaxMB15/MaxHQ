// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether an agent sees an image you attach or gets only its file path,
// and what the composer says when it gets the path.

import { t } from "../i18n";

import type { Account, ModelChoice } from "../bindings";
import { modelWithoutImages } from "../agent-view/controls";
import { imagesNote, takesImages } from "../settings/acp-agents";

/**
 * What the composer says under an attached image the agent gets by its
 * path only, or null when it sees the image. That's when its CLI takes no
 * images, or its model `model`, from `choices`, can't read them.
 */
export const imageNote = (
	account: Account | undefined,
	model: string | null,
	choices: readonly ModelChoice[]
): string | null => {
	if (account && !takesImages(account)) return imagesNote(account.label);
	const name = modelWithoutImages(model, choices);
	return name === null
		? null
		: t("{{name}} can't see images. The agent gets the file path instead.", { name: name });
};
