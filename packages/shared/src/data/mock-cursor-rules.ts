// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Cursor rule calls of the sample data and of tests. Cursor on the Mac
// keeps three shell rules an earlier MaxHQ's Always allow saved, and any
// other device's maxd is too old to read them. A Remove lasts until the
// app reloads.

import type { CursorRuleList, DeviceId } from "../bindings";
import type { Result } from "./request-source";
import type { CursorRuleCalls } from "../settings/cursor-rules";
import { MAC } from "./mock-view";

const PATH = "~/.cursor/cli-config.json";

const TOO_OLD: Result<CursorRuleList> = { status: "ok", data: { status: "too_old" } };

/** The Cursor rule calls of a `maxd` from before it read them. */
export const noCursorRules: CursorRuleCalls = {
	cursorRules: async () => TOO_OLD,
};

/** The Cursor rule calls over the sample rules. */
export const mockCursorRules = (): CursorRuleCalls => {
	const saved = new Map<DeviceId, string[]>([
		[MAC, ["Shell(git status)", "Shell(pnpm test)", "Shell(cargo build)"]],
	]);
	return {
		cursorRules: async (device, call) => {
			const rules = saved.get(device);
			if (!rules) return TOO_OLD;
			if (call.kind === "remove") {
				if (!call.rule.startsWith("Shell(")) {
					return {
						status: "error",
						error: { kind: "failed", message: "maxd removes only Shell(...) rules." },
					};
				}
				saved.set(
					device,
					rules.filter((rule) => rule !== call.rule)
				);
			}
			const left = [...(saved.get(device) ?? [])];
			return { status: "ok", data: { status: "listed", rules: { path: PATH, rules: left } } };
		},
	};
};
