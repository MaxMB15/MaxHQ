// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: resolving a media
// source and finding the pictures in Markdown, with the parts of the link,
// file link, path, media preview and video helpers they use. MaxHQ has no
// project root to join a relative path to: it goes
// to the agent's device as written, and maxd reads it from the agent's
// folder. AVI and MKV are left out, since the webview can't play them, so
// the list matches what the app's media cache keeps. A `blob:` URL and a
// `data:` video are left out too, since nothing here makes one and the
// content policy wouldn't load the video. URLs are read by hand rather
// than with `URL`, which React Native only partly has.

/** Whether a Markdown target is a picture or a video. */
export type MediaKind = "image" | "video";

/** A picture or video a reply names, and where its bytes come from. */
export type MediaSource = {
	readonly kind: MediaKind;
	readonly mime: string;
	/** A name for labels, from the end of the path or URL. */
	readonly name: string;
	/** The `#` part the reply wrote, such as where a video starts. */
	readonly fragment: string;
} & (
	| {
			/** A `data:` picture, whose bytes are in the text itself. */
			readonly access: "inline";
			readonly uri: string;
	  }
	| {
			/** A page on the web. Loading one on sight would tell its server you read the reply. */
			readonly access: "web";
			readonly uri: string;
	  }
	| {
			/** A file on the agent's device, absolute or from the agent's folder. */
			readonly access: "device";
			readonly path: string;
	  }
);

const DIRECT = /^(?:https?:|data:|\/\/)/i;
const SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;
const POSITION = /:(\d+)(?::(\d+))?$/;
const SLASH_DRIVE = /^\/[A-Za-z]:[\\/]/;
const FILE_URL = /^file:(?:\/\/([^/?#]*))?([^?#]*)/i;
const AUTHORITY = /^(?:https?:)?\/\/[^/?#]*/i;

const IMAGE_TYPES = new Map([
	[".avif", "image/avif"],
	[".bmp", "image/bmp"],
	[".heic", "image/heic"],
	[".heif", "image/heif"],
	[".gif", "image/gif"],
	[".ico", "image/x-icon"],
	[".jpeg", "image/jpeg"],
	[".jpg", "image/jpeg"],
	[".png", "image/png"],
	[".svg", "image/svg+xml"],
	[".tif", "image/tiff"],
	[".tiff", "image/tiff"],
	[".webp", "image/webp"],
]);

const VIDEO_TYPES = new Map([
	[".m4v", "video/mp4"],
	[".mov", "video/quicktime"],
	[".mp4", "video/mp4"],
	[".ogv", "video/ogg"],
	[".webm", "video/webm"],
]);

// Sound isn't shown, and these keep a sound file out of the picture
// fallback for an embed with an extension nothing else knows.
const AUDIO_TYPES = new Map([
	[".mp3", "audio/mpeg"],
	[".wav", "audio/wav"],
	[".ogg", "audio/ogg"],
	[".oga", "audio/ogg"],
	[".flac", "audio/flac"],
	[".aac", "audio/aac"],
	[".m4a", "audio/mp4"],
	[".opus", "audio/ogg"],
	[".aiff", "audio/aiff"],
	[".webm-audio", "audio/webm"],
	[".weba", "audio/webm"],
]);

export const safeDecode = (value: string): string => {
	try {
		return decodeURIComponent(value);
	} catch {
		return value;
	}
};

export const normalizeDestination = (value: string): string => {
	const trimmed = value.trim();
	return trimmed.startsWith("<") && trimmed.endsWith(">") ? trimmed.slice(1, -1) : trimmed;
};

export const splitSearchAndHash = (value: string): { path: string; hash: string } => {
	const hashAt = value.indexOf("#");
	const withSearch = hashAt >= 0 ? value.slice(0, hashAt) : value;
	const hash = hashAt >= 0 ? value.slice(hashAt) : "";
	const searchAt = withSearch.indexOf("?");
	return { path: searchAt >= 0 ? withSearch.slice(0, searchAt) : withSearch, hash };
};

/** Browser URL parsers write `C:/foo` as `/C:/foo` for file URLs. */
export const stripSlashDrive = (path: string): string =>
	SLASH_DRIVE.test(path) ? path.slice(1) : path;

export const isWindowsAbsolute = (path: string): boolean =>
	path.startsWith("\\\\") || /^[a-zA-Z]:([/\\]|$)/.test(path);

/**
 * Two leading separators name a share on another machine, and Windows signs
 * in to it with the user's credentials, so a path an agent wrote never
 * reaches one.
 */
export const isNetworkPath = (path: string): boolean => /^[\\/]{2}/.test(path);

/** The last part of a path, without the separators after it. */
export const fileBasename = (path: string): string => {
	const trimmed = path.replace(/[/\\]+$/, "");
	if (trimmed.length === 0) return path;
	const at = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
	return at >= 0 ? trimmed.slice(at + 1) : trimmed;
};

/** A path without the `:line:column` an agent may write after it. */
const withoutPosition = (path: string): string => {
	const match = path.match(POSITION);
	return match ? path.slice(0, -match[0].length) : path;
};

/** The path a `file:` URL names on its own machine, or null for any other. */
export const fileUrlPath = (href: string): string | null => {
	const match = FILE_URL.exec(href);
	if (!match) return null;
	const host = (match[1] ?? "").toLowerCase();
	if (host !== "" && host !== "localhost") return null;
	const path = match[2] ?? "";
	if (path.length === 0) return null;
	return stripSlashDrive(safeDecode(path));
};

/** The path of a web URL, without its scheme, host, search or `#` part. */
const webPath = (uri: string): string => splitSearchAndHash(uri.replace(AUTHORITY, "")).path;

const extensionOf = (name: string): string | null => {
	const at = name.lastIndexOf(".");
	return at < 0 ? null : name.slice(at).toLowerCase();
};

/** The type a literal extension names, without decoding or trimming it. */
const typeOfExtension = (extension: string | null): string | null => {
	if (extension === null || !/^\.[a-z0-9-]+$/.test(extension)) return null;
	return (
		IMAGE_TYPES.get(extension) ??
		VIDEO_TYPES.get(extension) ??
		AUDIO_TYPES.get(extension) ??
		null
	);
};

/** The type of a `data:` or web URL, from its media type or the end of its path. */
const typeOfUrl = (uri: string): string | null => {
	const inline = /^data:((?:image|video|audio)\/[\w.+-]+)[;,]/i.exec(uri)?.[1];
	if (inline) return inline.toLowerCase();
	if (/^data:/i.test(uri)) return null;
	return typeOfExtension(extensionOf(fileBasename(safeDecode(webPath(uri)))));
};

type Classified =
	{ readonly tag: "url"; readonly uri: string } | { readonly tag: "file"; readonly path: string };

const onDevice = (path: string): Classified | null =>
	isNetworkPath(path) ? null : { tag: "file", path };

/**
 * Whether a Markdown target is a URL or a file, and the file's decoded
 * path. A path never reaches the webview: only the device reads it.
 */
const classify = (value: string): Classified | null => {
	const source = normalizeDestination(value);
	if (source.length === 0 || source.startsWith("#") || source.startsWith("?")) return null;
	if (DIRECT.test(source)) return { tag: "url", uri: source };
	if (/^file:/i.test(source)) {
		const path = fileUrlPath(source);
		return path === null ? null : onDevice(path);
	}
	const path = stripSlashDrive(safeDecode(splitSearchAndHash(source).path));
	if (path.length === 0) return null;
	if (path.startsWith("/") || isWindowsAbsolute(path)) return onDevice(path);
	if (SCHEME.test(path) || path.startsWith("~/") || path.startsWith("~\\")) return null;
	return { tag: "file", path };
};

/**
 * What a Markdown image target is, or null when it's nothing MaxHQ shows,
 * such as a sound, a page with no picture's extension, or a path outside
 * what a reply may point at. `imageEmbed` takes a web URL with no
 * extension, which image syntax names, as a picture.
 */
export const resolveMediaSource = (
	source: string | null | undefined,
	{ imageEmbed = false }: { readonly imageEmbed?: boolean } = {}
): MediaSource | null => {
	if (source === null || source === undefined) return null;
	const classified = classify(source);
	if (classified === null) return null;

	const fragment = splitSearchAndHash(normalizeDestination(source)).hash;
	if (classified.tag === "file") {
		// A local path is decoded already, so a literal #, ? or % stays.
		const path = withoutPosition(classified.path);
		const name = fileBasename(path);
		const mime = typeOfExtension(extensionOf(name));
		if (mime === null || mime.startsWith("audio/")) return null;
		const kind = mime.startsWith("video/") ? "video" : "image";
		return { kind, mime, name: name || kind, fragment, access: "device", path };
	}

	const { uri } = classified;
	const inline = /^data:/i.test(uri);
	const mime = typeOfUrl(uri) ?? (imageEmbed && !inline ? "image/*" : null);
	if (mime === null || mime.startsWith("audio/")) return null;
	const kind = mime.startsWith("video/") ? "video" : "image";
	if (inline && kind === "video") return null;
	const name = inline ? "" : safeDecode(fileBasename(webPath(uri)));
	return inline
		? { kind, mime, name: kind, fragment: "", access: "inline", uri }
		: {
				kind,
				mime,
				name: name || kind,
				fragment,
				access: "web",
				uri: uri.startsWith("//") ? `https:${uri}` : uri,
			};
};
