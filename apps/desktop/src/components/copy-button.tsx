// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Copying to the clipboard, and saying for a moment whether it worked. The
// button's words change, and a screen reader hears the same words.

import { t, useI18n } from "@maxhq/shared/i18n";

import { Check, CircleAlert, Copy } from "lucide-react";

import { copyLabel, type CopyState, useCopyWith } from "@maxhq/shared/lib/copy";

export type { CopyState };
export { copyLabel };

// The clipboard isn't there outside a secure page, which counts as a failure too.
const writeText = (text: string) => navigator.clipboard.writeText(text);

/** How the last copy went, and the function that copies. */
export const useCopy = (
	write: (text: string) => Promise<unknown> = writeText
): [CopyState, (text: string) => void] => useCopyWith(write);

/** Says how a copy went to a screen reader, and nothing on screen. */
export const CopyStatus = ({ state }: { state: CopyState }) => {
	useI18n();
	return (
		<span role="status" className="sr-only">
			{copyLabel(state, "")}
		</span>
	);
};

const LOOK =
	"ml-auto inline-flex h-[22px] items-center gap-1.5 rounded-[5px] px-[7px] text-[11.5px] text-muted-foreground hover:bg-accent hover:text-foreground";

/** Copies text, and says so for a moment. */
export const CopyButton = ({
	text,
	label = t("Copy"),
	icon = true,
	title,
	className = LOOK,
	write,
}: {
	text: string;
	/** Writes the copy, such as with more types than plain text. Plain text unless given. */
	write?: (text: string) => Promise<unknown>;
	/** What it says until it's clicked. */
	label?: string;
	/** Whether the copy icon shows before the label. The check always shows once copied. */
	icon?: boolean;
	title?: string;
	/** The button's look, the code block's small one unless given. */
	className?: string;
}) => {
	useI18n();

	const [state, copy] = useCopy(write);
	return (
		<>
			<button type="button" title={title} className={className} onClick={() => copy(text)}>
				{state === "copied" ? (
					<Check size={12} aria-hidden />
				) : state === "failed" ? (
					<CircleAlert size={12} aria-hidden />
				) : icon ? (
					<Copy size={12} aria-hidden />
				) : null}
				{copyLabel(state, label)}
			</button>
			<CopyStatus state={state} />
		</>
	);
};
