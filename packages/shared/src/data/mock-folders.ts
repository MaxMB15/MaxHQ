// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The folder calls of the sample data and of tests. The Mac and devbox
// each have a small made-up home folder with a few checkouts in it, which
// the path fields suggest from and the folder browser walks. A new folder
// stays until the app reloads. Folders are listed as `maxd` lists them:
// folders first, each group by name ignoring case, and a folder that
// isn't there answers with no entries.

import type { DeviceId, FolderCheckout, FolderEntry, ProjectId } from "../bindings";
import { baseName, expandHome, joinPath, parentOf } from "../folders/path";
import type { FolderCalls } from "../folders/requests";
import { DEVBOX, MAC } from "./mock-view";

/** The folder calls of a `maxd` from before folders. */
export const noFolders: FolderCalls = {
	listFolder: async () => ({ status: "ok", data: { status: "too_old" } }),
	makeFolder: async () => ({
		status: "ok",
		data: { status: "refused", reason: "This device's maxd can't make folders yet." },
	}),
};

/** One name in a sample folder. */
type Node = { readonly folder: boolean; readonly checkout: FolderCheckout | null };

/** A sample device: its home and what each folder holds, by path then name. */
type Disk = { readonly home: string; readonly folders: Map<string, Map<string, Node>> };

const git = (project: ProjectId, branch: string | null): Node => ({
	folder: true,
	checkout: { project, branch },
});

const FOLDER: Node = { folder: true, checkout: null };
const FILE: Node = { folder: false, checkout: null };

/** Puts `node` at `path`, and each folder above it that isn't there yet. */
const put = (disk: Disk, path: string, node: Node) => {
	const parent = parentOf(path);
	if (parent === null) return;
	if (!disk.folders.has(parent)) put(disk, parent, FOLDER);
	const names = disk.folders.get(parent) ?? new Map<string, Node>();
	disk.folders.set(parent, names.set(baseName(path), node));
	if (node.folder && !disk.folders.has(path)) disk.folders.set(path, new Map());
};

/** A sample device with `home`, holding `paths` relative to it. */
const disk = (home: string, paths: Record<string, Node>): Disk => {
	const made: Disk = { home, folders: new Map([["/", new Map()]]) };
	put(made, home, FOLDER);
	for (const [path, node] of Object.entries(paths)) put(made, joinPath(home, path), node);
	return made;
};

const sampleDisks = (): Map<DeviceId, Disk> =>
	new Map([
		[
			MAC,
			disk("/Users/me", {
				".config": FOLDER,
				"Code/Github/Multi/MaxAIManager": git("proj-maxhq", "main"),
				"Code/Github/Multi/MaxAIManager/apps": FOLDER,
				"Code/Github/Multi/MaxAIManager/crates": FOLDER,
				"Code/Github/Multi/MaxAIManager/package.json": FILE,
				"Code/Github/Multi/MaxAIManager/README.md": FILE,
				"Code/Github/Multi/Comet": git("proj-comet", "main"),
				"Code/Github/Multi/Comet/src": FOLDER,
				"Code/scratch": FOLDER,
				Desktop: FOLDER,
				Documents: FOLDER,
				Downloads: FOLDER,
				"notes.md": FILE,
			}),
		],
		[
			DEVBOX,
			disk("/home/dev", {
				".config": FOLDER,
				".profile": FILE,
				"code/Mosaic": git("proj-mosaic", "main"),
				"code/Mosaic/Cargo.toml": FILE,
				"code/DuskTools": git("proj-dusk-tools", "dev"),
				"code/notes": FOLDER,
				"code/scratch": FOLDER,
				"code/run.sh": FILE,
				"bin/agent-cli": FILE,
			}),
		],
	]);

/** Folders first, each group by name ignoring case. */
const ordered = (names: Map<string, Node>, files: boolean, hidden: boolean): FolderEntry[] =>
	[...names]
		.filter(([name, node]) => (files || node.folder) && (hidden || !name.startsWith(".")))
		.map(([name, node]) => ({ name, ...node }))
		.sort(
			(a, b) =>
				Number(b.folder) - Number(a.folder) ||
				a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
		);

const unknownDevice = (device: DeviceId) => ({
	status: "error" as const,
	error: { kind: "not_found" as const, what: `device ${device}` },
});

/** The folder calls over each sample device's home folder. */
export const mockFolders = (): FolderCalls => {
	const disks = sampleDisks();
	/** `path` with `~` read and any trailing slash dropped. */
	const full = (disk: Disk, path: string) => {
		const read = expandHome(path, disk.home);
		return read.length > 1 ? read.replace(/\/+$/u, "") : read;
	};
	return {
		listFolder: async (device, path, { files, hidden }) => {
			const disk = disks.get(device);
			if (!disk) return unknownDevice(device);
			const at = full(disk, path);
			const names = disk.folders.get(at);
			return {
				status: "ok",
				data: {
					status: "listed",
					listing: {
						path: at,
						home: disk.home,
						exists: names !== undefined,
						entries: names ? ordered(names, files, hidden) : [],
						truncated: false,
					},
				},
			};
		},
		makeFolder: async (device, path) => {
			const disk = disks.get(device);
			if (!disk) return unknownDevice(device);
			const at = full(disk, path);
			const parent = parentOf(at);
			const refuse = (reason: string) => ({
				status: "ok" as const,
				data: { status: "refused" as const, reason },
			});
			if (parent === null || !disk.folders.has(parent))
				return refuse(`${parent ?? at} isn't there.`);
			if (disk.folders.get(parent)?.has(baseName(at)))
				return refuse(`${at} is there already.`);
			put(disk, at, FOLDER);
			return { status: "ok", data: { status: "done" } };
		},
	};
};
