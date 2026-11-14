// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Text you might type somewhere else, with a quiet copy icon beside it: a
// value in its own mono field, or a few words of code or a web address
// inside a sentence. The icon sits in a muted colour at rest and turns to a
// check for a moment once it has copied. The text itself doesn't select,
// like the rest of the app's chrome, so the icon is the way to copy it.

import { Check, CircleAlert, Copy } from "lucide-react";

import { cn } from "@/lib/utils";
import { t, useI18n } from "@maxhq/shared/i18n";
import { copyLabel } from "@maxhq/shared/lib/copy";

import { CopyStatus, useCopy } from "./copy-button";
import { OutLink } from "./out-link";

/** An icon that copies `text`. */
export const CopyIcon = ({
	text,
	label,
	className,
}: {
	text: string;
	/** What it says to a screen reader and on hover, "Copy" unless given. */
	label?: string;
	className?: string;
}) => {
	useI18n();
	const [state, copy] = useCopy();
	const said = copyLabel(state, label ?? t("Copy"));
	const Icon = state === "copied" ? Check : state === "failed" ? CircleAlert : Copy;
	return (
		<>
			<button
				type="button"
				aria-label={said}
				title={said}
				onClick={() => copy(text)}
				className={cn(
					"inline-grid size-[22px] shrink-0 place-items-center rounded-[5px] align-middle text-muted-foreground hover:bg-accent hover:text-foreground",
					state === "copied" && "text-status-ready-text",
					className
				)}
			>
				<Icon aria-hidden size={12} strokeWidth={2} />
			</button>
			<CopyStatus state={state} />
		</>
	);
};

/** A value in a mono field, with its copy icon at the end. */
export const CopyField = ({
	value,
	label,
	copyLabel: copyName,
	className,
}: {
	value: string;
	/** The field's name for a screen reader. */
	label: string;
	/** The icon's name, "Copy" unless given. */
	copyLabel?: string;
	className?: string;
}) => {
	useI18n();
	return (
		<div
			className={cn(
				"flex min-w-0 items-center gap-1 rounded-[7px] border border-border bg-subpanel py-[3px] pr-[3px] pl-2.5",
				className
			)}
		>
			<code
				aria-label={label}
				title={value}
				className="min-w-0 flex-1 truncate font-mono text-xs text-foreground"
			>
				{value}
			</code>
			<CopyIcon text={value} label={copyName} />
		</div>
	);
};

/**
 * Code inside a sentence, or with `link` a web address that opens in the
 * browser, with a small copy icon after it.
 */
export const InlineCopy = ({
	text,
	link,
	className,
}: {
	text: string;
	link?: boolean;
	className?: string;
}) => {
	useI18n();
	// The icon stays with the last word, however the text wraps.
	return (
		<span className="whitespace-nowrap">
			{link ? (
				<OutLink href={text} className={cn("break-all whitespace-normal", className)} />
			) : (
				<code
					className={cn(
						"rounded-[4px] bg-secondary px-1 py-px font-mono text-[11.5px] break-all whitespace-normal text-foreground",
						className
					)}
				>
					{text}
				</code>
			)}
			<CopyIcon
				text={text}
				label={link ? t("Copy URL") : undefined}
				className="-my-1 ml-0.5 size-[18px]"
			/>
		</span>
	);
};
