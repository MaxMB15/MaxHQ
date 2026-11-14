// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The files that open in MaxHQ's own file viewer rather than an editor:
// Markdown, pictures, PDFs, 3D models, tables, Office documents, and code
// and plain text, which the viewer shows read-only with the editor one
// click away. The bytes come through the media cache, so the cache on each
// client keeps every extension here (crates/client/src/media.rs). STEP and
// other CAD formats stay out, since reading them takes code the app can't
// ship. Files that hold secrets, such as .env, .pem and .key, stay out too,
// so a click never puts one on screen. HTML and the like are code here:
// the viewer shows their text and never runs them.

import { t } from "../i18n";

import { type AgentId, SHOW_ANYWAY_SINCE } from "../bindings";
import type { ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/** What kind of viewer file a path names. */
export type ViewerKind = "markdown" | "image" | "pdf" | "model" | "table" | "office" | "code";

const KINDS: Readonly<Record<ViewerKind, readonly string[]>> = {
	markdown: ["md", "markdown"],
	image: [
		"avif",
		"bmp",
		"gif",
		"heic",
		"heif",
		"ico",
		"jpeg",
		"jpg",
		"png",
		"svg",
		"tif",
		"tiff",
		"webp",
	],
	pdf: ["pdf"],
	model: ["glb", "gltf", "stl", "obj", "3mf"],
	table: ["csv", "tsv"],
	office: ["docx", "xlsx", "pptx", "odt", "ods", "odp"],
	// Every language the chat's highlighter knows, by the extensions its
	// files use, then plain text.
	code: [
		"bash",
		"c",
		"cc",
		"cjs",
		"cpp",
		"css",
		"cts",
		"cxx",
		"diff",
		"dockerfile",
		"go",
		"h",
		"hh",
		"hpp",
		"htm",
		"html",
		"hxx",
		"java",
		"js",
		"json",
		"jsonc",
		"jsx",
		"kt",
		"kts",
		"mjs",
		"mts",
		"nix",
		"patch",
		"py",
		"pyi",
		"rb",
		"rs",
		"sh",
		"sql",
		"swift",
		"toml",
		"ts",
		"tsx",
		"xml",
		"yaml",
		"yml",
		"zsh",
		"cfg",
		"conf",
		"ini",
		"log",
		"text",
		"txt",
	],
};

const BY_EXTENSION: ReadonlyMap<string, ViewerKind> = new Map(
	(Object.entries(KINDS) as [ViewerKind, readonly string[]][]).flatMap(([kind, list]) =>
		list.map((extension) => [extension, kind] as const)
	)
);

/** Every extension the viewer opens, lowercase. */
export const VIEWER_EXTENSIONS: readonly string[] = [...BY_EXTENSION.keys()];

/** A file's name, the last part of its path. */
export const fileName = (path: string): string => path.split(/[/\\]/).pop() || path;

/** The lowercase extension at the end of a path's name, or "" when it has none. */
export const extensionOf = (path: string): string => {
	const name = fileName(path);
	const dot = name.lastIndexOf(".");
	return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
};

/** What the viewer makes of a file, or null when it opens in an editor. */
export const viewerKind = (path: string): ViewerKind | null =>
	BY_EXTENSION.get(extensionOf(path)) ?? null;

/** The largest file the viewer opens. A larger one opens in the editor. */
export const VIEWER_LIMIT = 256 * 1024 * 1024;

/** The largest code or text file the viewer shows. A larger one opens in the editor. */
export const CODE_LIMIT = 4 * 1024 * 1024;

/** The most bytes the viewer fetches for a file at `path`. */
export const viewerLimit = (path: string): number =>
	viewerKind(path) === "code" ? CODE_LIMIT : VIEWER_LIMIT;

/**
 * Why Show anyway can't reach `agent`'s device, or null when it can. A
 * `maxd` from before Show anyway doesn't read the request, and one whose
 * version isn't known yet is taken as older.
 */
export const showAnywayBlocked = (view: ViewState, agent: AgentId): string | null => {
	const workspace = view.tables.agent.get(agent)?.workspace;
	const device = workspace ? view.tables.workspace.get(workspace)?.device : undefined;
	if (!device) return t("MaxHQ doesn't know which device this agent is on yet.");
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	if (protocol !== null && protocol >= SHOW_ANYWAY_SINCE) return null;
	return t(
		"Update maxd on {{deviceName_view__device}} to show files outside the agent's folders.",
		{ deviceName_view__device: deviceName(view, device) }
	);
};
