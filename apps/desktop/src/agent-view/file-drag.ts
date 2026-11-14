// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pulling a picture or file out of a timeline, to drop on a composer. The
// drag is the webview's own, so it gets the system's drag picture and
// Escape, and anywhere outside MaxHQ it drops as the file's name. It
// carries where the file is: the device, the agent whose timeline it's in,
// which can read it there, and its path. The sidebar's pointer drag stays
// inside the page, so a drop elsewhere couldn't take the name. A handler
// here mustn't stop the dragstart, since the drop router notes what a drag
// carries once it reaches the document.

import type { DragEvent as ReactDragEvent } from "react";

import { useServices } from "@/app/services";
import type { AgentId, AttachmentKind, DeviceId } from "@maxhq/shared/bindings";
import { useRow } from "@maxhq/shared/agent-view/context";

/** The data type a timeline file drags as. */
export const FILE_DRAG_TYPE = "application/x-maxhq-file";

/** A file pulled out of a timeline. */
export type DraggedFile = {
	/** The device the file is on. */
	readonly device: DeviceId;
	/** The agent whose timeline it's in, which can read it on that device. */
	readonly agent: AgentId;
	/** Where it is on that device: absolute, or from the agent's folder. */
	readonly path: string;
	readonly name: string;
	readonly kind: AttachmentKind;
	readonly mime: string | null;
	/** The size, when the timeline knows it. */
	readonly bytes: number | null;
};

/** What a timeline file says about itself, without where it is. */
export type FileToDrag = Pick<DraggedFile, "path" | "name" | "kind" | "mime" | "bytes">;

/** What an element spreads to drag a file, or nothing. */
export type FileDrag = {
	readonly draggable?: true;
	readonly onDragStart?: (event: ReactDragEvent<HTMLElement>) => void;
};

const KINDS: ReadonlySet<string> = new Set<AttachmentKind>(["image", "pdf", "file", "folder"]);

const text = (value: unknown): value is string => typeof value === "string" && value !== "";

/** The file a drag carries, or null for anything that isn't one. */
export const readDraggedFile = (raw: string | null | undefined): DraggedFile | null => {
	if (!raw) return null;
	let value: unknown;
	try {
		value = JSON.parse(raw);
	} catch {
		return null;
	}
	if (typeof value !== "object" || value === null) return null;
	const { device, agent, path, name, kind, mime, bytes } = value as Record<string, unknown>;
	if (!text(device) || !text(agent) || !text(path) || !text(name)) return null;
	if (typeof kind !== "string" || !KINDS.has(kind)) return null;
	if (mime !== null && typeof mime !== "string") return null;
	if (bytes !== null && typeof bytes !== "number") return null;
	return {
		device: device as DeviceId,
		agent: agent as AgentId,
		path,
		name,
		kind: kind as AttachmentKind,
		mime,
		bytes,
	};
};

/** Puts `file` on a drag that's starting. */
export const startFileDrag = (data: DataTransfer, file: DraggedFile) => {
	data.effectAllowed = "copy";
	data.setData(FILE_DRAG_TYPE, JSON.stringify(file));
	data.setData("text/plain", file.name);
};

/**
 * Lets an element in a row drag `file` out of the timeline, or nothing for
 * null. Spread the result on the element. `onStart` runs once the drag is
 * on its way.
 */
export const useFileDrag = (file: FileToDrag | null, onStart?: () => void): FileDrag => {
	const { agent } = useRow();
	const services = useServices();
	if (file === null) return {};
	return {
		draggable: true,
		onDragStart: (event) => {
			// The device is read as the drag starts, so an agent that moved goes from where it is.
			const { view } = services.view.getState();
			const record = view.tables.agent.get(agent);
			const device = record && view.tables.workspace.get(record.workspace)?.device;
			if (device === undefined) {
				event.preventDefault();
				return;
			}
			startFileDrag(event.dataTransfer, {
				device,
				agent,
				path: file.path,
				name: file.name,
				kind: file.kind,
				mime: file.mime,
				bytes: file.bytes,
			});
			onStart?.();
		},
	};
};
