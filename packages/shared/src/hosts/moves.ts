// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Moves of containers the old scripts made into ones MaxHQ runs, as their
// cards show them: each step in order with what it changes, and what you
// can do next. Nothing orders the moves, so each card runs on its own.

import { t } from "../i18n";

import type { DeviceId, Move, MoveStep, Timestamp, Workspace } from "../bindings";
import { formatWhen } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";
import { isMade } from "../workspaces/state";
import { unsavedOf } from "./unsaved";

/** The steps of a move, in order. */
export const MOVE_STEPS: readonly MoveStep[] = [
	"look",
	"copy",
	"converge",
	"switch",
	"keep",
	"retire",
];

/** What each step is called. */
export const STEP_LABEL: Readonly<Record<MoveStep, string>> = {
	get look() {
		return t("Look");
	},
	get copy() {
		return t("Copy");
	},
	get converge() {
		return t("Set up");
	},
	get switch() {
		return t("Switch");
	},
	get keep() {
		return t("Keep the original");
	},
	get retire() {
		return t("Retire the original");
	},
};

/** Whether a device's `maxd` moves workspaces into MaxHQ. */
export const offersMoves = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.moves === true;

/** The moves on a device, or on every device with none. */
export const movesOn = (view: ViewState, device?: DeviceId): Move[] =>
	all(view, "move").filter((m) => device === undefined || m.device === device);

/** The move of a container, if one has started. */
export const moveOf = (view: ViewState, device: DeviceId, instance: string): Move | undefined =>
	all(view, "move").find((m) => m.device === device && m.source === instance);

/** How one step stands on a card. */
export type StepStatus = "done" | "ready" | "running" | "failed" | "todo";

/** How each step stands, in words. */
export const STEP_STATUS_LABEL: Readonly<Record<StepStatus, string>> = {
	get done() {
		return t("Done");
	},
	get ready() {
		return t("Waiting for you");
	},
	get running() {
		return t("Running");
	},
	get failed() {
		return t("Failed");
	},
	get todo() {
		return t("Not yet");
	},
};

/** One step's line on a card. */
export type StepView = {
	readonly step: MoveStep;
	readonly label: string;
	readonly status: StepStatus;
	/** What it changes, from the look. */
	readonly changes: readonly string[];
	/** Why it failed, or what it's doing. */
	readonly note: string | null;
};

/** Something you can do to a move from its card. */
export type MoveAction =
	| { readonly kind: "run"; readonly step: MoveStep; readonly label: string }
	| { readonly kind: "move_back"; readonly label: string }
	| { readonly kind: "retire"; readonly label: string };

/** Whether a move is over, gone back or retired. */
export const isOver = (move: Move): boolean =>
	move.state.state === "moved_back" || move.state.state === "retired";

/** Each step of a move, with how it stands. */
export const moveSteps = (move: Move): StepView[] => {
	const at = MOVE_STEPS.indexOf(move.step);
	return MOVE_STEPS.map((step, index) => {
		const changes = move.plan?.steps.find((s) => s.step === step)?.changes ?? [];
		const base = { step, label: STEP_LABEL[step], changes, note: null };
		if (index < at || (index === at && move.state.state === "retired")) {
			return { ...base, status: "done" };
		}
		if (index > at || move.state.state === "moved_back") return { ...base, status: "todo" };
		switch (move.state.state) {
			case "running":
				return { ...base, status: "running", note: move.state.doing };
			case "failed":
				return { ...base, status: "failed", note: move.state.reason };
			default:
				return { ...base, status: "ready" };
		}
	});
};

/**
 * What you can do next: run the step the move is at, try a failed one
 * again, copy again before the switch when the original changed since
 * the copy, and go back or retire early while the original is kept.
 */
export const moveActions = (move: Move): MoveAction[] => {
	const { state } = move.state;
	if (state === "running" || isOver(move)) return [];
	if (move.step === "keep") {
		return [
			{ kind: "move_back", label: t("Move back") },
			{ kind: "retire", label: t("Retire now") },
		];
	}
	const run: MoveAction = {
		kind: "run",
		step: move.step,
		label: state === "failed" ? t("Try again") : STEP_LABEL[move.step],
	};
	if (move.step === "switch" && move.source_changed) {
		return [{ kind: "run", step: "copy", label: t("Copy again") }, run];
	}
	return [run];
};

/** A line under a card on how the move stands, such as when the keep ends. */
export const moveNote = (move: Move, now: Timestamp): string | null => {
	switch (move.state.state) {
		case "moved_back":
			return t("Moved back. {{move_source}} runs again.", { move_source: move.source });
		case "retired":
			return t("Moved. {{move_source}} is deleted.", { move_source: move.source });
		default:
			if (move.step === "switch" && move.source_changed) {
				return t("{{move_source}} changed since the copy.", { move_source: move.source });
			}
			if (move.kept_until !== null && move.step === "keep") {
				return t("{{move_source}} is kept until {{formatWhen_move_kept_until__now}}.", {
					move_source: move.source,
					formatWhen_move_kept_until__now: formatWhen(move.kept_until, now),
				});
			}
			return null;
	}
};

/**
 * The containers on a device that MaxHQ didn't make and that have no move
 * yet, which a card offers to move into MaxHQ.
 */
export const movableContainers = (view: ViewState, device: DeviceId): Workspace[] =>
	all(view, "workspace").filter(
		(w) =>
			w.device === device &&
			w.location.type === "container" &&
			!isMade(w) &&
			!moveOf(view, device, w.location.instance)
	);

/**
 * What retiring a move's original would lose, for its confirm: the
 * original's unsaved work when MaxHQ knows it, else a warning to check.
 */
export const retireLoses = (view: ViewState, move: Move, now: Timestamp): string => {
	const original = all(view, "workspace").find(
		(w) =>
			w.device === move.device &&
			w.location.type === "container" &&
			w.location.instance === move.source
	);
	const unsaved = original ? unsavedOf(original, now) : null;
	if (!unsaved) {
		return t(
			"MaxHQ can't read unsaved work in {{move_source}}, so check it before you delete it.",
			{ move_source: move.source }
		);
	}
	return unsaved.any
		? t("{{move_source}} has {{unsaved_text}}. Deleting it loses that.", {
				move_source: move.source,
				unsaved_text: unsaved.text,
			})
		: t("{{move_source}} has nothing unsaved.", { move_source: move.source });
};
