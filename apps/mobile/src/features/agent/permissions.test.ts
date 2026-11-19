// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Permission, PermissionId } from "@maxhq/shared/bindings";
import { NO_LIMITS } from "@maxhq/shared/data/source";
import type { ActionResult } from "@maxhq/shared/data/source";
import { agent } from "@maxhq/shared/test/build";

import {
	createPermissionStore,
	heldPermission,
	loadPermissions,
	type PermissionSource,
	revokeHeld,
	revokeQuestion,
} from "./permissions";

const allowed: Permission = {
	id: "perm-1",
	holder: { kind: "agent", agent: "a1" },
	home: "mac",
	devices: ["devbox"],
	vendors: ["codex"],
	accounts: null,
	places: null,
	depth: 1,
	limits: { ...NO_LIMITS, max_running: 2 },
	test_kinds: [],
	leave_container: "never",
};

const lead = agent({ id: "a1", control: "held" });

// A source that lists `list` and answers a revoke with `answer`.
const sourceOf = (list: Permission[], answer: ActionResult | Error) => {
	const revoked: PermissionId[] = [];
	const source: PermissionSource = {
		listPermissions: async () => ({ status: "ok", data: list }),
		revokePermission: async (id) => {
			revoked.push(id);
			if (answer instanceof Error) throw answer;
			if (answer.status === "ok") list.splice(0, list.length);
			return answer;
		},
	};
	return { source, revoked };
};

describe("an agent's permission on the phone", () => {
	it("finds the one a lead MaxHQ runs holds, not a sub-agent's or a watched one's", () => {
		expect(heldPermission([allowed], lead)?.id).toBe("perm-1");
		expect(heldPermission(null, lead)).toBeUndefined();
		expect(heldPermission([allowed], { ...lead, parent: "a0" })).toBeUndefined();
		expect(heldPermission([allowed], { ...lead, control: "observed" })).toBeUndefined();
	});

	it("keeps the list it had when a read fails", async () => {
		const store = createPermissionStore();
		store.setState({ list: [allowed] });
		await loadPermissions(store, {
			listPermissions: async () => ({
				status: "error",
				error: { kind: "failed", message: "offline" },
			}),
			revokePermission: async () => ({ status: "ok", data: { status: "done" } }),
		});
		expect(store.getState().list).toEqual([allowed]);
	});

	it("revokes it and reads the list again", async () => {
		const store = createPermissionStore();
		const { source, revoked } = sourceOf([allowed], { status: "ok", data: { status: "done" } });
		await loadPermissions(store, source);
		expect(heldPermission(store.getState().list, lead)).toBeDefined();
		expect(await revokeHeld(store, source, allowed)).toBeNull();
		expect(revoked).toEqual(["perm-1"]);
		expect(heldPermission(store.getState().list, lead)).toBeUndefined();
	});

	it("says why a revoke failed, in the desktop's words when the call throws", async () => {
		const store = createPermissionStore();
		const refused = sourceOf([allowed], {
			status: "ok",
			data: { status: "refused", reason: "not from a phone" },
		});
		expect(await revokeHeld(store, refused.source, allowed)).toBe("Not from a phone.");
		const thrown = sourceOf([allowed], new Error("gone"));
		expect(await revokeHeld(store, thrown.source, allowed)).toBe(
			"Couldn't revoke it. Try again."
		);
		expect(heldPermission(store.getState().list, lead)?.id).toBe("perm-1");
	});

	it("asks first, with what it may run now", () => {
		expect(revokeQuestion(allowed, 0)).toEqual({
			title: "Revoke its permission?",
			detail: "2 at once. It can't start agents after this.",
		});
	});
});
