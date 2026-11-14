// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A callout in a message: a quote that starts with `[!NOTE]` shows its icon
// and its name or title above the text, in its colour.

import { t as translate } from "@maxhq/shared/i18n";
import {
	Bug,
	CircleCheck,
	CircleHelp,
	CircleX,
	FileText,
	Flame,
	Info,
	Lightbulb,
	ListChecks,
	ListOrdered,
	type LucideIcon,
	MessageSquareWarning,
	OctagonAlert,
	Quote,
	TriangleAlert,
} from "lucide-react";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

import type { AlertKind } from "./markdown-plugins";

export type Tone = "blue" | "green" | "purple" | "amber" | "red" | "grey";

const LOOKS: Readonly<Record<AlertKind, { label: string; icon: LucideIcon; tone: Tone }>> = {
	note: {
		get label() {
			return translate("Note");
		},
		icon: Info,
		tone: "blue",
	},
	abstract: {
		get label() {
			return translate("Summary");
		},
		icon: FileText,
		tone: "blue",
	},
	info: {
		get label() {
			return translate("Info");
		},
		icon: Info,
		tone: "blue",
	},
	todo: {
		get label() {
			return translate("To do");
		},
		icon: ListChecks,
		tone: "blue",
	},
	tip: {
		get label() {
			return translate("Tip");
		},
		icon: Lightbulb,
		tone: "green",
	},
	success: {
		get label() {
			return translate("Success");
		},
		icon: CircleCheck,
		tone: "green",
	},
	question: {
		get label() {
			return translate("Question");
		},
		icon: CircleHelp,
		tone: "amber",
	},
	important: {
		get label() {
			return translate("Important");
		},
		icon: MessageSquareWarning,
		tone: "purple",
	},
	warning: {
		get label() {
			return translate("Warning");
		},
		icon: TriangleAlert,
		tone: "amber",
	},
	caution: {
		get label() {
			return translate("Caution");
		},
		icon: OctagonAlert,
		tone: "red",
	},
	failure: {
		get label() {
			return translate("Failed");
		},
		icon: CircleX,
		tone: "red",
	},
	danger: {
		get label() {
			return translate("Danger");
		},
		icon: Flame,
		tone: "red",
	},
	bug: {
		get label() {
			return translate("Bug");
		},
		icon: Bug,
		tone: "red",
	},
	example: {
		get label() {
			return translate("Example");
		},
		icon: ListOrdered,
		tone: "purple",
	},
	quote: {
		get label() {
			return translate("Quote");
		},
		icon: Quote,
		tone: "grey",
	},
};

const EDGES: Readonly<Record<Tone, string>> = {
	blue: "border-primary",
	green: "border-status-ready",
	purple: "border-status-linked",
	amber: "border-status-busy",
	red: "border-destructive",
	grey: "border-track",
};

const INKS: Readonly<Record<Tone, string>> = {
	blue: "text-primary",
	green: "text-status-ready",
	purple: "text-status-linked",
	amber: "text-status-busy",
	red: "text-destructive",
	grey: "text-muted-foreground",
};

// Raw HTML can name any kind, such as `constructor`, so only its own count.
const lookOf = (kind: string) =>
	Object.hasOwn(LOOKS, kind) ? LOOKS[kind as AlertKind] : LOOKS.note;

/** A callout's colour. */
export const alertTone = (kind: string): Tone => lookOf(kind).tone;

/** The classes of the quote that holds a callout. */
export const alertEdge = (kind: string): string => EDGES[lookOf(kind).tone];

/** A callout's top line: its title, or its name when it has none. */
export const AlertTitle = ({ kind, children }: { kind: string; children: ReactNode }) => {
	const { label, icon: Icon, tone } = lookOf(kind);
	const titled = Array.isArray(children) ? children.length > 0 : Boolean(children);
	return (
		<div className={cn("flex items-center gap-1.5 font-semibold", INKS[tone])}>
			<Icon aria-hidden size={14} strokeWidth={2} className="shrink-0" />
			{titled ? <span>{children}</span> : <span data-find-skip>{label}</span>}
		</div>
	);
};
