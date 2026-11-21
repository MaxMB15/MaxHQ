// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pieces the settings screens share: a card with a header, a row with
// its words on the left, buttons, fields, the note that says how a save
// went, a copy icon for an error's words, and a button that asks once more
// before it does anything.

import { type ReactNode, useState } from "react";

import { CopyIcon } from "@/components/copy-field";
import { cn } from "@/lib/utils";

import type { Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The look every settings button shares. */
export const BUTTON =
	"inline-flex h-[30px] shrink-0 items-center justify-center gap-1.5 rounded-[7px] px-3 text-[12.5px] whitespace-nowrap disabled:opacity-50";

/** The button that does the form's job. */
export const PRIMARY = cn(
	BUTTON,
	"bg-primary font-[650] text-primary-foreground shadow-lit enabled:hover:brightness-110"
);

/** Any other button. */
export const SECONDARY = cn(
	BUTTON,
	"border border-input bg-secondary text-foreground enabled:hover:bg-accent"
);

/** A button that throws something away. */
export const DANGER = cn(
	BUTTON,
	"border border-status-error/45 bg-status-error/12 font-[600] text-status-error-text enabled:hover:bg-status-error/20"
);

/** A text field. */
export const FIELD =
	"h-8 w-full rounded-[7px] border border-border bg-subpanel px-2.5 text-[12.5px] text-foreground outline-none placeholder:text-subtle-foreground focus:border-primary focus:shadow-focus disabled:opacity-60";

/** A field's name above it. */
export const Label = ({ children }: { children: ReactNode }) => {
	useI18n();
	return <span className="text-[11.5px] font-[550] text-secondary-foreground">{children}</span>;
};

/** A card with a title and an optional hint on the right. */
export const Section = ({
	title,
	hint,
	label,
	id,
	children,
	className,
}: {
	title: string;
	hint?: ReactNode;
	/** The name a screen reader hears, when it isn't the title. */
	label?: string;
	/** The element's ID, for the settings search to scroll to. */
	id?: string;
	children: ReactNode;
	className?: string;
}) => {
	useI18n();
	return (
		<section
			id={id}
			aria-label={label ?? title}
			className={cn("overflow-hidden rounded-[11px] border border-track bg-card", className)}
		>
			<div className="flex items-baseline justify-between gap-3 border-b border-sidebar-border px-4 py-3">
				<h2 className="text-[13.5px] font-[650] text-foreground">{title}</h2>
				{hint ? (
					<span className="selectable text-[11.5px] text-muted-foreground">{hint}</span>
				) : null}
			</div>
			{children}
		</section>
	);
};

/**
 * One row in a card: its name and what it means, then its control. The line
 * under the name often holds a value, such as an address or an error, so it
 * selects. The name doesn't.
 */
export const Row = ({
	title,
	sub,
	children,
	className,
}: {
	title: ReactNode;
	sub?: ReactNode;
	children?: ReactNode;
	className?: string;
}) => {
	useI18n();
	return (
		<div
			className={cn(
				"flex items-center gap-4 border-t border-sidebar-border px-4 py-3 first:border-t-0",
				className
			)}
		>
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span className="text-[12.5px] text-foreground">{title}</span>
				{sub ? (
					<span className="selectable text-xs text-muted-foreground">{sub}</span>
				) : null}
			</div>
			{children ? <div className="flex shrink-0 items-center gap-2.5">{children}</div> : null}
		</div>
	);
};

/** How a save went. */
export const StatusNote = ({ note, className }: { note: Note | null; className?: string }) => {
	useI18n();
	return note ? (
		<span
			role="status"
			className={cn(
				"selectable text-[11.5px]",
				note.error ? "text-status-error-text" : "text-muted-foreground",
				className
			)}
		>
			{note.text}
		</span>
	) : null;
};

/**
 * A copy icon after an error's words, for a report or a search, so a long
 * error copies whole without a drag across it.
 */
export const ErrorCopy = ({ text }: { text: string }) => {
	useI18n();
	return (
		<CopyIcon text={text} label={translate("Copy error")} className="-my-1 ml-1 size-[18px]" />
	);
};

/**
 * A button that asks once more before it acts: the first click shows the
 * question with a second button that goes ahead, and a Keep button that
 * puts things back.
 */
export const ConfirmButton = ({
	label,
	question,
	confirm,
	onConfirm,
	disabled,
}: {
	label: string;
	question: string;
	confirm: string;
	onConfirm: () => void;
	disabled?: boolean;
}) => {
	useI18n();
	const [asking, setAsking] = useState(false);
	if (!asking) {
		return (
			<button
				type="button"
				className={SECONDARY}
				disabled={disabled}
				onClick={() => setAsking(true)}
			>
				{label}
			</button>
		);
	}
	return (
		<span className="flex items-center gap-2">
			<span className="text-[11.5px] text-secondary-foreground">{question}</span>
			<button type="button" className={SECONDARY} onClick={() => setAsking(false)}>
				{translate("Keep")}
			</button>
			<button
				type="button"
				className={DANGER}
				disabled={disabled}
				onClick={() => {
					setAsking(false);
					onConfirm();
				}}
			>
				{confirm}
			</button>
		</span>
	);
};
