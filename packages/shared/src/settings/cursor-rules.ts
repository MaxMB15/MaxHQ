// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The shell rules Cursor saved in its own settings on each device. Always
// allow in an earlier MaxHQ also told Cursor to keep the rule, which Cursor
// writes as `Shell(git status)` in its allow list, and from then on it runs
// every command that starts with those words without asking MaxHQ.
// Settings lists them device by device, with Remove. Only the `Shell(...)`
// entries show, and maxd leaves the rest of Cursor's settings as they are.

import type { CursorRuleCall, CursorRuleList, DeviceId } from "../bindings";
import { failureOf } from "../data/failure";
import type { Ask, Result } from "../data/request-source";
import type { ViewState } from "../view/replica";

import { accountDevices, installed, type AccountDevice } from "./accounts";

/** The Cursor rule calls of a data source. */
export type CursorRuleCalls = {
	/**
	 * Lists the `Shell(...)` rules Cursor saved on `device`, or removes one
	 * and lists the rest. Answers `too_old` from a `maxd` that can't read
	 * them.
	 */
	readonly cursorRules: (
		device: DeviceId,
		call: CursorRuleCall
	) => Promise<Result<CursorRuleList>>;
};

/** The Cursor rule calls, each one request over `ask`. */
export const cursorRuleCalls = (ask: Ask): CursorRuleCalls => ({
	cursorRules: (device, call) => ask({ kind: "cursor_rules", device, call }),
});

/** What a device's part of the list shows. */
export type CursorRuleState =
	| { readonly kind: "loading" }
	/** Its `maxd` can't read Cursor's rules yet. */
	| { readonly kind: "too_old" }
	| { readonly kind: "failed"; readonly message: string }
	| {
			readonly kind: "listed";
			/** The settings file, with `~` for the device's home folder. */
			readonly path: string;
			readonly rules: readonly string[];
	  };

/** What a device's answer shows. */
export const ruleState = (result: Result<CursorRuleList>): CursorRuleState => {
	if (result.status === "error") return { kind: "failed", message: failureOf(result) ?? "" };
	const list = result.data;
	return list.status === "too_old"
		? { kind: "too_old" }
		: { kind: "listed", path: list.rules.path, rules: list.rules.rules };
};

/**
 * What the list shows once a Remove answered `next`: the rules left, or
 * the list as it was and why the rule stayed.
 */
export const afterRemove = (
	before: CursorRuleState,
	next: CursorRuleState
): { readonly state: CursorRuleState; readonly error: string | null } =>
	next.kind === "failed" ? { state: before, error: next.message } : { state: next, error: null };

const SHELL = "Shell(";

/** The words a rule lets through, such as `git status` for `Shell(git status)`. */
export const ruleWords = (rule: string): string =>
	rule.startsWith(SHELL) && rule.endsWith(")") ? rule.slice(SHELL.length, -1) : rule;

/**
 * The devices the list has a part for: each linked one, this Mac first,
 * unless it reports that it has no Cursor CLI.
 */
export const cursorRuleDevices = (view: ViewState): AccountDevice[] =>
	accountDevices(view).filter((d) => installed(view, d.device, "cursor") !== false);
