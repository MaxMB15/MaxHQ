// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The webview's copy of the view, kept the way `Replica` in
// crates/protocol/src/state.rs keeps it. A snapshot replaces everything. A
// delta applies only when it is the next version. One already held is a
// repeat, and anything further ahead is a gap the caller fixes by loading a
// fresh snapshot. An entity or change of a kind this build doesn't hold,
// such as one from a newer core, is left out and the rest still applies.

import type { EntityKey as GeneratedKey, Seq } from "../bindings";
import type { Delta, Entity, EntityKey, Snapshot } from "../fleet/wire";

/** The kinds of entity the view holds. */
export type EntityKind = Entity["entity"];

/** The entity of one kind, without the other kinds in the union. */
export type EntityOf<K extends EntityKind> = Extract<Entity, { entity: K }>;

/**
 * The ID one project's settings, skills, command rules or notes, or one
 * phone, on one device are keyed by.
 */
const settingsId = (project: string, device: string): string => `${project}/${device}`;

/**
 * The ID an entity is keyed by. Device settings, tools and tasks, the
 * skill library, the host report, the ACP registry, network access, hidden
 * projects and the dispatcher use their device's ID, project settings,
 * skills, command rules and notes their project's and device's together, a
 * phone its own and its device's, a phone book its phone's, and a
 * dispatcher's chat its agent's.
 */
export const idOf = (entity: Entity): string => {
	switch (entity.entity) {
		case "device_settings":
		case "device_tools":
		case "device_tasks":
		case "skill_library":
		case "host_report":
		case "acp_registry":
		case "network_access":
		case "hidden_projects":
		case "dispatcher":
			return entity.device;
		case "project_settings":
		case "project_skills":
		case "command_rules":
		case "project_notes":
			return settingsId(entity.project, entity.device);
		case "phone":
			return settingsId(entity.id, entity.device);
		case "phone_book":
			return entity.phone;
		case "dispatcher_chat":
			return entity.agent;
		default:
			return entity.id;
	}
};

/** The ID a key names, the same one `idOf` gives the entity it names. */
export const idOfKey = (key: EntityKey): string => {
	switch (key.entity) {
		case "project_settings":
		case "project_skills":
		case "command_rules":
		case "project_notes":
			return settingsId(key.id.project, key.id.device);
		case "phone":
			return settingsId(key.id.phone, key.id.device);
		default:
			return key.id;
	}
};

/** The key that names an entity, for removing it. */
export const keyOf = (entity: Entity): GeneratedKey => {
	switch (entity.entity) {
		case "project_settings":
		case "project_skills":
		case "command_rules":
		case "project_notes":
			return {
				entity: entity.entity,
				id: { project: entity.project, device: entity.device },
			};
		case "phone":
			return { entity: "phone", id: { phone: entity.id, device: entity.device } };
		default:
			return { entity: entity.entity, id: idOf(entity) } as GeneratedKey;
	}
};

/** Every entity, one map per kind, keyed by ID. */
export type Tables = { readonly [K in EntityKind]: ReadonlyMap<string, EntityOf<K>> };

/** The view at one version. */
export type ViewState = {
	/** The version held, or null before the first snapshot. */
	readonly seq: Seq | null;
	/** The entities at that version. */
	readonly tables: Tables;
};

/** What applying a delta did. */
export type DeltaResult =
	/** The delta was the next version and the view moved to it. */
	| { readonly kind: "applied"; readonly view: ViewState }
	/** The view already held that version, so nothing changed. */
	| { readonly kind: "repeat" }
	/** The delta doesn't follow the version held. Load a snapshot. */
	| { readonly kind: "gap"; readonly have: Seq | null; readonly got: Seq };

const KINDS: readonly EntityKind[] = [
	"device",
	"device_settings",
	"device_tools",
	"device_tasks",
	"device_link",
	"network",
	"project",
	"workspace",
	"agent",
	"account",
	"attention",
	"listening_port",
	"forward",
	"project_settings",
	"credential",
	"acp_agent",
	"test_host",
	"test_lease",
	"workflow_run",
	"skill_library",
	"project_skills",
	"skill_proposal",
	"phone",
	"phone_book",
	"host_report",
	"move",
	"acp_registry",
	"network_access",
	"phone_session",
	"hidden_projects",
	"dispatcher",
	"dispatcher_chat",
	"command_rules",
	"project_notes",
	"app_key",
	"admission",
	"terminal",
];

const HELD: ReadonlySet<string> = new Set(KINDS);

/** Whether this build holds the kind. */
const held = (kind: string | undefined): kind is EntityKind => kind !== undefined && HELD.has(kind);

/** The kind a change is for, or undefined for an op this build doesn't know. */
const kindOf = (change: Delta["changes"][number]): string | undefined => {
	switch (change.op) {
		case "upsert":
			return change.value.entity;
		case "remove":
			return change.key.entity;
		default:
			return undefined;
	}
};

const emptyTables = (): Tables =>
	Object.fromEntries(KINDS.map((kind) => [kind, new Map()])) as unknown as Tables;

/** The view before the first snapshot. */
export const EMPTY_VIEW: ViewState = { seq: null, tables: emptyTables() };

type MutableTables = { [K in EntityKind]: Map<string, EntityOf<K>> };

const put = (tables: MutableTables, entity: Entity) => {
	(tables[entity.entity] as Map<string, Entity>).set(idOf(entity), entity);
};

/** Replaces the whole view with the snapshot. */
export const applySnapshot = (snapshot: Snapshot): ViewState => {
	const tables = emptyTables() as MutableTables;
	for (const entity of snapshot.entities) {
		if (held(entity.entity)) put(tables, entity);
	}
	return { seq: snapshot.seq, tables };
};

/** Applies the next delta, or says why it can't. Never changes `view`. */
export const applyDelta = (view: ViewState, delta: Delta): DeltaResult => {
	if (view.seq === null || delta.seq > view.seq + 1) {
		return { kind: "gap", have: view.seq, got: delta.seq };
	}
	if (delta.seq <= view.seq) return { kind: "repeat" };

	// Copy only the maps this delta touches, so selectors on the others keep
	// seeing the same objects.
	const tables = { ...view.tables } as MutableTables;
	const copied = new Set<EntityKind>();
	const table = (kind: EntityKind) => {
		if (!copied.has(kind)) {
			copied.add(kind);
			(tables as Record<EntityKind, Map<string, Entity>>)[kind] = new Map(
				view.tables[kind] as ReadonlyMap<string, Entity>
			);
		}
		return (tables as Record<EntityKind, Map<string, Entity>>)[kind];
	};

	for (const change of delta.changes) {
		if (!held(kindOf(change))) continue;
		if (change.op === "upsert") {
			table(change.value.entity).set(idOf(change.value), change.value);
		} else {
			table(change.key.entity).delete(idOfKey(change.key));
		}
	}
	return { kind: "applied", view: { seq: delta.seq, tables } };
};

/** The entity with this key, if the view holds it. */
export const lookup = <K extends EntityKind>(
	view: ViewState,
	key: EntityKey & { entity: K }
): EntityOf<K> | undefined => view.tables[key.entity].get(idOfKey(key)) as EntityOf<K> | undefined;

/** Every entity of one kind, in no particular order. */
export const all = <K extends EntityKind>(view: ViewState, kind: K): EntityOf<K>[] => [
	...(view.tables[kind].values() as Iterable<EntityOf<K>>),
];
