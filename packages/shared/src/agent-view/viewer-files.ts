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

/** The folder a path's file is in, as the path gives it, or "" for a bare name. */
export const folderOf = (path: string): string => {
	const at = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
	return at < 0 ? "" : at === 0 ? path.slice(0, 1) : path.slice(0, at);
};

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

/** The first line a unified diff changes in the new file, if it says. */
export const editLine = (patch: string | null): number | undefined => {
	const hunk = patch ? /^@@ -\d+(?:,\d+)? \+(\d+)/m.exec(patch) : null;
	return hunk ? Number(hunk[1]) : undefined;
};

/** The first line of a read's span, such as 10 for "10-40". */
export const readLine = (lines: string | null): number | undefined => {
	const first = lines ? /^\s*(\d+)/.exec(lines) : null;
	return first ? Number(first[1]) : undefined;
};

/** A file a link points at on the agent's device, and the line it names. */
export type FileLink = { readonly path: string; readonly line?: number };

const LINK_SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;
const DRIVE = /^[A-Za-z]:[\\/]/;
const FILE_LINK = /^file:(?:\/\/([^/?#]*))?([^?#]*)/i;
const LINE_HASH = /^#L(\d+)(?:C\d+)?(?:-L?\d+(?:C\d+)?)?$/i;
const LINE_SUFFIX = /:(\d+)(?::\d+)?$/;

const decoded = (value: string): string => {
	try {
		return decodeURIComponent(value);
	} catch {
		return value;
	}
};

// `path` from `folder`, with its `.` and `..` parts worked out, or null when
// it climbs out of the top.
const fromFolder = (folder: string, path: string): string | null => {
	const separator = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
	const parts: string[] = [];
	for (const part of `${folder}/${path}`.split(/[/\\]/)) {
		if (part === "" || part === ".") continue;
		if (part !== "..") parts.push(part);
		else if (parts.length === 0 || (parts.length === 1 && DRIVE.test(`${parts[0]}/`)))
			return null;
		else parts.pop();
	}
	return (folder.startsWith("/") ? "/" : "") + parts.join(separator);
};

/**
 * The file a Markdown link points at on the agent's device, with the line
 * it names, or null when it's anything else: a web page, an email, another
 * scheme, a place on the page, or a file the viewer doesn't open. A
 * `file://` URL, an absolute path and a relative path all count. The line
 * comes from `#L12` or `#L12-L20`, or from `:12` or `:12:3` after the path.
 * A relative path starts from `folder`, as a Markdown file's links start
 * from the file's own. Without one, maxd reads it from the agent's first
 * folder and refuses one with a `..` part, so a link with one is left alone.
 */
export const localFileLink = (url: string, folder?: string): FileLink | null => {
	const trimmed = url.trim();
	const target =
		trimmed.startsWith("<") && trimmed.endsWith(">") ? trimmed.slice(1, -1) : trimmed;
	if (target === "" || target.startsWith("#") || target.startsWith("?")) return null;
	const hashAt = target.indexOf("#");
	const hash = hashAt >= 0 ? target.slice(hashAt) : "";

	let path: string;
	if (/^file:/i.test(target)) {
		const match = FILE_LINK.exec(target);
		const host = (match?.[1] ?? "").toLowerCase();
		if (!match || (host !== "" && host !== "localhost")) return null;
		path = decoded(match[2] ?? "");
		// `file:///C:/x` names `C:/x`.
		if (/^\/[A-Za-z]:[\\/]/.test(path)) path = path.slice(1);
	} else {
		// A share on another machine, or a home folder maxd doesn't expand.
		if (/^(?:[\\/]{2}|~)/.test(target)) return null;
		const before = hashAt >= 0 ? target.slice(0, hashAt) : target;
		const queryAt = before.indexOf("?");
		const bare = queryAt >= 0 ? before.slice(0, queryAt) : before;
		// `a.ts:12` is a file and a line, not a scheme.
		if (!DRIVE.test(bare) && LINK_SCHEME.test(bare.replace(LINE_SUFFIX, ""))) return null;
		path = decoded(bare);
	}

	let line: number | undefined;
	const fromHash = LINE_HASH.exec(hash);
	if (fromHash) line = Number(fromHash[1]);
	const suffix = LINE_SUFFIX.exec(path);
	if (suffix) {
		path = path.slice(0, -suffix[0].length);
		line ??= Number(suffix[1]);
	}

	if (folder && path !== "" && !/^[/\\]/.test(path) && !DRIVE.test(path)) {
		const joined = fromFolder(folder, path);
		if (joined === null) return null;
		path = joined;
	}
	if (path === "" || path.split(/[/\\]/).includes("..")) return null;
	if (viewerKind(path) === null) return null;
	return line !== undefined && line >= 1 ? { path, line } : { path };
};

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
