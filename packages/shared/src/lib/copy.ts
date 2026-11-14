// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Copying to the clipboard, and saying for a moment whether it worked. Each
// client passes its own way to write to the clipboard.

import { t } from "../i18n";

import { useEffect, useState } from "react";

/** How the last copy went, until it's forgotten a moment later. */
export type CopyState = "idle" | "copied" | "failed";

// How long each one shows. A failure stays longer, since it asks you to
// copy by hand.
const SHOWN_MS = { copied: 1500, failed: 3000 } as const;

/** What a copy control says: `idle` until it's used, then how it went. */
export const copyLabel = (state: CopyState, idle: string): string =>
	state === "copied" ? t("Copied") : state === "failed" ? t("Couldn't copy") : idle;

/** How the last copy went, and the function that copies through `write`. */
export const useCopyWith = (
	write: (text: string) => Promise<unknown>
): [CopyState, (text: string) => void] => {
	// A new object on every use, so a second one starts the wait again.
	const [shown, setShown] = useState<{ readonly state: CopyState }>({ state: "idle" });

	useEffect(() => {
		if (shown.state === "idle") return;
		const timer = setTimeout(() => setShown({ state: "idle" }), SHOWN_MS[shown.state]);
		return () => clearTimeout(timer);
	}, [shown]);

	const copy = (text: string) => {
		const failed = (error: unknown) => {
			console.warn(t("Couldn't copy"), error);
			setShown({ state: "failed" });
		};
		try {
			write(text).then(() => setShown({ state: "copied" }), failed);
		} catch (error) {
			failed(error);
		}
	};

	return [shown.state, copy];
};
