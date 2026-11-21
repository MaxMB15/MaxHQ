// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The fleet's protocol types, from the bindings.

import type { ActionOutcome } from "../bindings";

export type {
	AppInfo,
	BundledBuild,
	Capabilities,
	ContainerUse,
	Delta,
	Device,
	DeviceLink,
	DeviceSettings,
	DeviceTools,
	Entity,
	EntityKey,
	FleetProgress,
	FleetStatus,
	HostLoad,
	KeepAwake,
	Placement,
	ProcessUse,
	RouteChoice,
	SetupHint,
	SetupHost,
	SetupPhase,
	SetupProgress,
	SetupSecret,
	SetupStep,
	Snapshot,
	Tool,
	ToolInfo,
	ToolOwner,
	ToolPlace,
	ToolState,
	ToolTarget,
	ToolUpdate,
	UseBucket,
} from "../bindings";

/** Names one setup run. */
export type SetupId = string;

/** What a device's `maxd`, agents and containers used over a window. */
export type Diagnostics = Pick<
	Extract<ActionOutcome, { status: "diagnostics" }>,
	"buckets" | "top" | "containers" | "error"
>;
