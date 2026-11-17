// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether an agent may run agents, as the phone reads it. The phone lists
// every connected device's permissions when an agent's screen opens, and
// keeps the list in one store, so the More sheet and the details screen
// agree. It can revoke a permission, which maxd takes from a phone, but
// not grant or change one, which only the Mac does.

import { t } from "@maxhq/shared/i18n";

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { failureOf } from "@maxhq/shared/agent-view/answer";
import { canAllow, heldBy, limitsLine } from "@maxhq/shared/agent-tools/scope";
import type { Agent, Permission, PermissionId, Timestamp } from "@maxhq/shared/bindings";
import type { ActionResult, PermissionsResult } from "@maxhq/shared/data/source";

/** What the permissions need from the source. */
export type PermissionSource = {
	readonly listPermissions: (device: null) => Promise<PermissionsResult>;
	readonly revokePermission: (permission: PermissionId) => Promise<ActionResult>;
};

/** The permissions read last, or null before the first read came back. */
export type PermissionList = { readonly list: readonly Permission[] | null };

export const createPermissionStore = (): StoreApi<PermissionList> =>
	createStore<PermissionList>(() => ({ list: null }));

/** The phone's one list. */
export const permissions = createPermissionStore();

/** Reads the list again. A read that fails keeps the list it had. */
export const loadPermissions = async (
	store: StoreApi<PermissionList>,
	source: PermissionSource
): Promise<void> => {
	try {
		const result = await source.listPermissions(null);
		if (result.status === "ok") store.setState({ list: result.data });
	} catch {
		// Offline or refused: the screen shows what it had.
	}
};

/** The permission the agent holds, once the list is read. */
export const heldPermission = (
	list: readonly Permission[] | null,
	agent: Pick<Agent, "id" | "control" | "parent">
): Permission | undefined => (list && canAllow(agent) ? heldBy(list, agent.id) : undefined);

/** The permission the agent holds, from the phone's list. */
export const useHeldPermission = (agent: Agent): Permission | undefined =>
	useStore(permissions, (s) => heldPermission(s.list, agent));

/** What Revoke asks first: what it may run now, and that it stops. */
export const revokeQuestion = (
	held: Permission,
	now: Timestamp
): { readonly title: string; readonly detail: string } => ({
	title: t("Revoke its permission?"),
	detail: t("{{limits}}. It can't start agents after this.", {
		limits: limitsLine(held.limits, now),
	}),
});

/**
 * Revokes `held`, then reads the list again. Resolves to what went wrong,
 * in the desktop's words, or null.
 */
export const revokeHeld = async (
	store: StoreApi<PermissionList>,
	source: PermissionSource,
	held: Permission
): Promise<string | null> => {
	let failure: string | null;
	try {
		failure = failureOf(await source.revokePermission(held.id));
	} catch {
		failure = t("Couldn't {{what}}. Try again.", { what: t("revoke it") });
	}
	if (failure === null)
		store.setState((s) => ({ list: s.list?.filter((p) => p.id !== held.id) ?? null }));
	await loadPermissions(store, source);
	return failure;
};
