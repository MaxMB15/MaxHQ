// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES. A picture or video
// on the agent's device comes through the media cache instead of a signed
// asset URL: a picture up to the cache's on-sight limit when it scrolls into
// view, anything larger and every video on a click. One on the web isn't
// loaded, since that would tell its server you read the reply, and shows as a
// link to open in the browser. The copy menu and the gallery are left out, and
// a click shows the picture whole instead. A picture on the device drags out
// as its file. A video doesn't, since its controls take the drag.

import {
	ExternalLink,
	Film,
	Image as ImageIcon,
	Play,
	RotateCw,
	TriangleAlert,
} from "lucide-react";
import {
	type KeyboardEvent as ReactKeyboardEvent,
	type MouseEvent as ReactMouseEvent,
	useEffect,
	useMemo,
	useState,
} from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { useRow } from "@maxhq/shared/agent-view/context";
import {
	cancelMedia,
	FULL_LIMIT,
	loadMedia,
	mediaFailed,
	PICTURE_LIMIT,
	useMedia,
} from "@maxhq/shared/agent-view/media";
import { type MediaKind, resolveMediaSource } from "@maxhq/shared/agent-view/media-source";

import { DownloadButton } from "./download";
import { type FileDrag, useFileDrag } from "./file-drag";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import { Spinner } from "./icons";
import { useInPictureSet } from "./picture-set";
import { PictureViewer, type Picture, useSeen } from "./picture-viewer";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const MEDIA_MAX_WIDTH = "max-w-[min(100%,30rem)]";
const MEDIA_BOUNDS = cn("max-h-[30rem]", MEDIA_MAX_WIDTH);
const MEDIA_LAYOUT = "inline-block!";
const MEDIA_FRAME = "rounded-lg border border-border/40";
const IMAGE_SIZE = cn("h-auto w-auto object-contain", MEDIA_BOUNDS);
const IMAGE_FRAME = cn(
	"aspect-video w-full overflow-hidden bg-muted/60",
	MEDIA_MAX_WIDTH,
	MEDIA_FRAME
);
const NOTE =
	"inline-flex! max-w-full items-center gap-1.5 rounded-md border border-border/40 bg-muted/40 px-2 py-1 align-middle text-xs text-muted-foreground";
const SMALL_BUTTON =
	"inline-flex shrink-0 cursor-pointer items-center gap-1 rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground";

const percent = (sent: number, total: number) =>
	total > 0 ? Math.min(100, Math.floor((sent / total) * 100)) : null;

// A picture that opens whole on a click, unless it's the face of a link.
const expandable = (onExpand: () => void, alt: string) => {
	const expand = (event: ReactMouseEvent | ReactKeyboardEvent) => {
		if (event.currentTarget.closest("a")) return;
		event.preventDefault();
		event.stopPropagation();
		onExpand();
	};
	return {
		role: "button" as const,
		tabIndex: 0,
		"aria-label": translate("Show {{value1}} full size", { value1: alt.trim() || "image" }),
		onClick: expand,
		onKeyDown: (event: ReactKeyboardEvent) => {
			if (event.key === "Enter" || event.key === " ") expand(event);
		},
	};
};

const UNAVAILABLE = {
	get image() {
		return translate("Image unavailable");
	},
	get video() {
		return translate("Video unavailable");
	},
} as const;

/** Why a picture or video in a reply doesn't show, with a way to try again. */
const MediaUnavailable = ({
	kind,
	alt,
	detail,
	onRetry,
}: {
	kind: MediaKind;
	alt: string;
	detail?: string;
	onRetry?: () => void;
}) => {
	useI18n();
	return (
		<span role="alert" className={NOTE}>
			<TriangleAlert aria-hidden className="size-3.5 shrink-0" />
			<span className="min-w-0">
				{[UNAVAILABLE[kind], alt, detail].filter(Boolean).join(" · ")}
			</span>
			{onRetry ? (
				<button
					type="button"
					aria-label={translate("Try loading {{value1}} again", { value1: alt || kind })}
					onClick={onRetry}
					className={SMALL_BUTTON}
				>
					<RotateCw size={12} strokeWidth={2} aria-hidden />
				</button>
			) : null}
		</span>
	);
};

/** A picture the webview can load, which opens whole on a click. */
const ShownImage = ({
	src,
	alt,
	name,
	picture,
	drag,
	onError,
}: {
	src: string;
	alt: string;
	name: string;
	picture: Picture;
	/** Set for a picture on the device, which drags out as its file. */
	drag?: FileDrag;
	onError?: () => void;
}) => {
	useI18n();
	const [open, setOpen] = useState(false);
	const { place, open: openInSet } = useInPictureSet({ name: alt || name, picture, drag });
	return (
		<>
			<img
				ref={place}
				src={src}
				alt={alt}
				decoding="async"
				draggable={false}
				className={cn(IMAGE_SIZE, MEDIA_LAYOUT, MEDIA_FRAME, "cursor-zoom-in align-middle")}
				onError={onError}
				{...expandable(openInSet ?? (() => setOpen(true)), alt || name)}
				{...drag}
			/>
			{openInSet ? null : (
				<PictureViewer
					open={open}
					onOpenChange={setOpen}
					name={alt || name}
					picture={picture}
					drag={drag}
				/>
			)}
		</>
	);
};

// A picture on the agent's device. One up to the on-sight limit comes when
// it scrolls into view, and a larger one waits for a click.
const DeviceImage = ({
	path,
	alt,
	name,
	fragment,
}: {
	path: string;
	alt: string;
	name: string;
	fragment: string;
}) => {
	useI18n();
	const { agent } = useRow();
	const { source } = useServices();
	const media = useMedia(agent, path);
	const [ref, seen] = useSeen<HTMLSpanElement>();
	useEffect(() => {
		if (seen) void loadMedia(source, agent, path, PICTURE_LIMIT);
	}, [seen, source, agent, path]);
	const label = alt || name;
	const bytes = media?.state === "large" ? media.size : null;
	const drag = useFileDrag({ path, name, kind: "image", mime: null, bytes });

	switch (media?.state) {
		case "ready":
			return (
				<ShownImage
					src={media.url + fragment}
					alt={alt}
					name={name}
					picture={{ kind: "url", url: media.url + fragment }}
					drag={drag}
					onError={() =>
						mediaFailed(
							agent,
							path,
							translate("Couldn't show {{name}}.", { name: name })
						)
					}
				/>
			);
		case "failed":
			return (
				<MediaUnavailable
					kind="image"
					alt={label}
					detail={media.reason}
					onRetry={() => void loadMedia(source, agent, path, FULL_LIMIT)}
				/>
			);
		case "large":
			return (
				<span {...drag} className={NOTE}>
					<ImageIcon aria-hidden className="size-3.5 shrink-0" />
					{media.size <= FULL_LIMIT ? (
						<button
							type="button"
							title={label}
							aria-label={translate("Show {{label}}, {{bytes}}", {
								label: label,
								bytes: formatBytes(media.size),
							})}
							onClick={() => void loadMedia(source, agent, path, FULL_LIMIT)}
							className="min-w-0 cursor-pointer truncate text-left hover:text-foreground"
						>
							{translate("Show picture, {{bytes}}", {
								bytes: formatBytes(media.size),
							})}
						</button>
					) : (
						<span title={label} className="min-w-0 truncate">
							{translate("Picture too large to show here, {{bytes}}", {
								bytes: formatBytes(media.size),
							})}
						</span>
					)}
					<DownloadButton path={path} name={name} className="h-[18px] min-w-[18px]" />
				</span>
			);
		default: {
			const done = media?.state === "loading" ? percent(media.sent, media.total) : null;
			return (
				<span
					ref={ref}
					role="status"
					aria-label={translate("Loading {{label}}", { label: label })}
					className={cn(MEDIA_LAYOUT, IMAGE_FRAME, "relative align-middle")}
				>
					<span className="absolute inset-0 flex items-center justify-center gap-1.5 text-[11.5px] text-muted-foreground tabular-nums">
						<Spinner size={12} />
						{done !== null ? `${done}%` : null}
					</span>
				</span>
			);
		}
	}
};

// A video on the agent's device. Nothing is asked for until a click, since
// even its size makes the device read the whole file, which can take a
// while for a large one on another machine. The size shows once the device
// has said it, with the progress.
const DeviceVideo = ({
	path,
	alt,
	name,
	fragment,
}: {
	path: string;
	alt: string;
	name: string;
	fragment: string;
}) => {
	useI18n();
	const { agent } = useRow();
	const { source } = useServices();
	const media = useMedia(agent, path);
	const label = alt || name;

	if (media?.state === "ready") {
		return (
			<video
				src={media.url + fragment}
				controls
				preload="metadata"
				aria-label={label}
				className={cn(
					MEDIA_LAYOUT,
					MEDIA_BOUNDS,
					MEDIA_FRAME,
					"w-full bg-media align-middle"
				)}
				onError={() =>
					mediaFailed(agent, path, translate("Couldn't play {{name}}.", { name: name }))
				}
			/>
		);
	}
	if (media?.state === "failed") {
		return (
			<MediaUnavailable
				kind="video"
				alt={label}
				detail={media.reason}
				onRetry={() => void loadMedia(source, agent, path, FULL_LIMIT)}
			/>
		);
	}

	let face;
	if (media?.state === "loading") {
		const done = percent(media.sent, media.total);
		face = (
			<span
				role="status"
				aria-label={translate("Loading {{label}}", { label: label })}
				className="flex items-center gap-2 rounded-md bg-media/55 px-2.5 py-1 text-[12px] text-media-foreground tabular-nums"
			>
				<Spinner size={12} />
				{done !== null
					? translate("{{done}}% of {{bytes}}", {
							done: done,
							bytes: formatBytes(media.total),
						})
					: null}
				<button
					type="button"
					onClick={() => cancelMedia(source, agent, path)}
					className="cursor-pointer rounded px-1 text-media-foreground/80 hover:text-media-foreground"
				>
					{translate("Cancel")}
				</button>
			</span>
		);
	} else if (media?.state === "large" && media.size > FULL_LIMIT) {
		face = (
			<span className="flex items-center gap-1.5 rounded-md bg-media/55 px-2.5 py-1 text-[12px] text-media-foreground">
				{translate("{{bytes}}, too large to play here", { bytes: formatBytes(media.size) })}
				<DownloadButton
					path={path}
					name={name}
					className="h-[18px] min-w-[18px] text-media-foreground hover:bg-media-foreground/10 hover:text-media-foreground"
				/>
			</span>
		);
	} else {
		face = (
			<button
				type="button"
				aria-label={translate("Play {{label}}", { label: label })}
				onClick={() => void loadMedia(source, agent, path, FULL_LIMIT)}
				className="flex cursor-pointer items-center gap-1.5 rounded-md bg-media/55 px-2.5 py-1 text-[12px] text-media-foreground tabular-nums hover:bg-media/70"
			>
				<Play size={12} strokeWidth={2.2} aria-hidden />
				{media?.state === "large"
					? translate("Load video, {{bytes}}", { bytes: formatBytes(media.size) })
					: translate("Load video")}
			</button>
		);
	}
	return (
		<span className={cn(MEDIA_LAYOUT, IMAGE_FRAME, "relative align-middle")}>
			<span className="absolute inset-0 flex items-center justify-center">{face}</span>
			<span className="absolute inset-x-0 bottom-0 flex items-center gap-1.5 truncate px-2.5 pb-1.5 text-[11px] text-muted-foreground">
				<Film size={12} strokeWidth={1.9} aria-hidden className="shrink-0" />
				<span className="truncate">{label}</span>
			</span>
		</span>
	);
};

// A picture or video on the web, as a link. Loading it here would tell its
// server you read the reply, so it opens in the browser instead.
const WebMedia = ({ uri, kind, label }: { uri: string; kind: MediaKind; label: string }) => {
	useI18n();
	const Icon = kind === "video" ? Film : ImageIcon;
	return (
		<a
			href={uri}
			target="_blank"
			rel="noreferrer"
			title={translate("{{uri}}\nPictures and videos from the web open in your browser.", {
				uri: uri,
			})}
			className={cn(NOTE, "hover:text-foreground")}
		>
			<Icon aria-hidden className="size-3.5 shrink-0" />
			<span className="min-w-0 truncate">{label}</span>
			<ExternalLink aria-hidden className="size-3 shrink-0" />
		</a>
	);
};

/**
 * A Markdown image: a picture or video on the agent's device, one written
 * into the reply, one on the web, or its alt text for anything else.
 */
export const MarkdownMedia = ({ src, alt = "" }: { src?: string; alt?: string }) => {
	useI18n();
	const media = useMemo(() => resolveMediaSource(src, { imageEmbed: true }), [src]);
	if (media === null) {
		return <span className="text-muted-foreground">{alt ? `[${alt}]` : "[image]"}</span>;
	}
	const label = alt.trim();
	switch (media.access) {
		case "inline":
			return (
				<ShownImage
					src={media.uri}
					alt={label}
					name={media.name}
					picture={{ kind: "url", url: media.uri }}
				/>
			);
		case "web":
			return <WebMedia uri={media.uri} kind={media.kind} label={label || media.name} />;
		case "device":
			return media.kind === "video" ? (
				<DeviceVideo
					key={media.path}
					path={media.path}
					alt={label}
					name={media.name}
					fragment={media.fragment}
				/>
			) : (
				<DeviceImage
					key={media.path}
					path={media.path}
					alt={label}
					name={media.name}
					fragment={media.fragment}
				/>
			);
	}
};
