// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Read-only questions about listening ports and forwards, per N-6: which
// ports an agent opened, which other ports listen in its workspace, what
// reaches them from this Mac, and what to call the workspace they're in.

import type {
	AgentId,
	DeviceId,
	Forward,
	ListeningPort,
	PortId,
	Timestamp,
	Workspace,
	WorkspaceId,
} from "../bindings";
import { deviceConnection } from "../status/connection";
import { all, type ViewState } from "../view/replica";
import { linkContext, linkForDevice } from "../view/select";

const byNumber = (a: ListeningPort, b: ListeningPort) =>
	a.port - b.port || a.id.localeCompare(b.id);

/**
 * Whether a device is somewhere other than this Mac. A device with no link
 * in the view counts as this Mac, the way M2 treated every agent.
 */
export const isRemoteDevice = (view: ViewState, device: DeviceId): boolean => {
	const link = linkForDevice(view, device);
	return link !== undefined && link.route.type !== "local";
};

/**
 * Whether a port on a device can be forwarded to this Mac: the device is
 * elsewhere and hasn't said it doesn't forward ports. A link that hasn't
 * heard from its device yet knows nothing, so the forward finds out.
 */
export const forwardsPorts = (view: ViewState, device: DeviceId): boolean =>
	isRemoteDevice(view, device) && linkForDevice(view, device)?.capabilities?.tunnels !== false;

/**
 * Whether this client can't reach a device right now, itself or through a
 * jump host. A forward to it could only fail, so nothing offers one.
 */
export const isUnreachable = (view: ViewState, device: DeviceId, now: Timestamp): boolean => {
	const link = linkForDevice(view, device);
	return (
		link !== undefined && deviceConnection(linkContext(view, link), now).look === "unreachable"
	);
};

/** Whether a port still listens, rather than lingering after it closed. */
export const isOpen = (port: ListeningPort): boolean => port.gone_since === null;

/** The ports an agent's processes listen on, in the order maxd first listed them. */
export const listedPorts = (view: ViewState, agent: AgentId): ListeningPort[] =>
	all(view, "listening_port").filter((port) => port.agent === agent && isOpen(port));

/** The ports an agent's processes listen on, lowest first. */
export const agentPorts = (view: ViewState, agent: AgentId): ListeningPort[] =>
	listedPorts(view, agent).sort(byNumber);

/** Every port that listens in a workspace, lowest first. */
export const workspacePorts = (view: ViewState, workspace: WorkspaceId): ListeningPort[] =>
	all(view, "listening_port")
		.filter((port) => port.workspace === workspace && isOpen(port))
		.sort(byNumber);

/** The forward that reaches a port, if one is open. */
export const forwardOf = (view: ViewState, port: PortId): Forward | undefined =>
	all(view, "forward").find((forward) => forward.port === port);

/** The workspace's short name: its container, or the folder it's in. */
export const placeName = (workspace: Workspace): string => {
	const { location } = workspace;
	if (location.type === "container") return location.instance;
	return location.path.split("/").filter(Boolean).at(-1) ?? location.path;
};

/** What a port's offer calls the workspace: its project, or else its place. */
export const subjectName = (view: ViewState, workspace: Workspace): string => {
	const project = workspace.project ? view.tables.project.get(workspace.project) : undefined;
	return project?.name ?? placeName(workspace);
};

/** A forward closed because its port did, for the notice that says so. */
export type ClosedForward = {
	/** The agent whose process held the port. */
	readonly agent: AgentId;
	/** The port's number in the workspace. */
	readonly port: number;
	/** The port this Mac listened on for it. */
	readonly localPort: number;
	/** The device the workspace is on. */
	readonly device: DeviceId;
};

/**
 * The forwards that stopped between two views because their port closed.
 * A port that lingers after it closed counts as closed, and one that was
 * already lingering in `before` was counted then.
 */
export const closedForwards = (before: ViewState, after: ViewState): ClosedForward[] =>
	all(before, "forward").flatMap((forward) => {
		const port = before.tables.listening_port.get(forward.port);
		if (!port || !isOpen(port) || port.agent === null) return [];
		const now = after.tables.listening_port.get(forward.port);
		if (now && isOpen(now)) return [];
		const workspace = before.tables.workspace.get(port.workspace);
		if (!workspace) return [];
		return [
			{
				agent: port.agent,
				port: port.port,
				localPort: forward.local_port,
				device: workspace.device,
			},
		];
	});
