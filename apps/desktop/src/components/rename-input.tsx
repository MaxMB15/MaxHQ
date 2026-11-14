// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: it opens with the
// title selected, Enter or leaving it saves, and Escape cancels. It holds its
// own text, as the header's field does, and takes the look of the place it
// stands in.

import { type KeyboardEvent, useRef } from "react";

import { useFieldTyping } from "@/composer/typing-prefs";
import { cn } from "@/lib/utils";

/** The field that takes a new name in place of a title. Mount it only while renaming. */
export const RenameInput = ({
	title,
	label,
	onCommit,
	onCancel,
	className,
}: {
	/** The title it starts with. */
	title: string;
	/** What a screen reader calls it. */
	label: string;
	/** Saves what's in it. */
	onCommit: (draft: string) => void;
	onCancel: () => void;
	className?: string;
}) => {
	// Enter and Escape blur the field as it unmounts, which mustn't save again.
	const settled = useRef(false);
	const typing = useFieldTyping();
	const keyDown = (event: KeyboardEvent<HTMLInputElement>) => {
		event.stopPropagation();
		if (event.nativeEvent.isComposing || event.keyCode === 229) return;
		if (event.key === "Enter") {
			event.preventDefault();
			settled.current = true;
			onCommit(event.currentTarget.value);
		} else if (event.key === "Escape") {
			event.preventDefault();
			settled.current = true;
			onCancel();
		}
	};
	return (
		<input
			autoFocus
			defaultValue={title}
			dir="auto"
			{...typing}
			aria-label={label}
			onFocus={(event) => event.currentTarget.select()}
			onKeyDown={keyDown}
			onBlur={(event) => {
				if (!settled.current) onCommit(event.currentTarget.value);
			}}
			onClick={(event) => event.stopPropagation()}
			onDoubleClick={(event) => event.stopPropagation()}
			className={cn(
				"min-w-0 flex-1 rounded-[4px] border border-input bg-card px-1 text-foreground-strong outline-none focus:border-ring",
				className
			)}
		/>
	);
};
