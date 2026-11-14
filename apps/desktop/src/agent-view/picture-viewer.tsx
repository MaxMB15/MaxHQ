// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Seeing a picture whole. A click on a picture in the timeline, whether one
// you attached, one a tool gave back or one a reply shows, opens it over
// the window at full size. One on the agent's device comes through the
// media cache, the same way for an agent here, on another device or in a
// container, and its small preview shows while the rest comes. One on the
// device also drags out as its file, onto a composer to attach it. Pictures
// shown together step left and right through one viewer (picture-set.tsx).

import { Dialog } from "@base-ui/react/dialog";
import { ChevronLeft, ChevronRight, Image, RotateCw, X } from "lucide-react";
import { type RefCallback, useEffect, useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { placeLabel } from "@maxhq/shared/agent-view/format";
import type { AttachmentRef } from "@maxhq/shared/bindings";
import { useRow } from "@maxhq/shared/agent-view/context";
import {
	cancelMedia,
	FULL_LIMIT,
	loadMedia,
	mediaFailed,
	useMedia,
} from "@maxhq/shared/agent-view/media";

import { DownloadButton } from "./download";
import { useInPictureSet } from "./picture-set";
import { type FileDrag, useFileDrag } from "./file-drag";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import { Spinner } from "./icons";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** A picture to show whole: one the webview can load, or one on the agent's device. */
export type Picture =
	| { readonly kind: "url"; readonly url: string }
	| {
			readonly kind: "device";
			readonly path: string;
			/** The small preview to show while it comes. */
			readonly thumbnail?: string | null;
	  };

/**
 * Whether an element has come near the visible part of the window, so a
 * picture fetches only when it's about to be seen. Without a way to tell,
 * it counts as seen.
 */
export const useSeen = <T extends Element>(): [RefCallback<T>, boolean] => {
	const [seen, setSeen] = useState(() => typeof IntersectionObserver === "undefined");
	const [node, setNode] = useState<T | null>(null);
	useEffect(() => {
		if (seen || !node) return;
		const observer = new IntersectionObserver(
			(entries) => {
				if (entries.some((entry) => entry.isIntersecting)) setSeen(true);
			},
			{ rootMargin: "200px" }
		);
		observer.observe(node);
		return () => observer.disconnect();
	}, [node, seen]);
	return [setNode, seen];
};

const percent = (sent: number, total: number) =>
	total > 0 ? Math.min(100, Math.floor((sent / total) * 100)) : null;

const WHOLE = "max-h-[86vh] max-w-[92vw] rounded-md object-contain";

const PLAIN_BUTTON =
	"inline-flex h-[26px] cursor-pointer items-center gap-1.5 rounded-md border border-media-foreground/15 bg-media/40 px-2.5 text-[12px] text-media-foreground hover:bg-media/60";

// A picture on the agent's device, fetched whole when the viewer opens.
const DeviceWhole = ({
	name,
	path,
	thumbnail,
}: {
	name: string;
	path: string;
	thumbnail: string | null;
}) => {
	useI18n();
	const { agent } = useRow();
	const { source } = useServices();
	const media = useMedia(agent, path);
	useEffect(() => {
		void loadMedia(source, agent, path, FULL_LIMIT);
	}, [source, agent, path]);

	if (media?.state === "ready") {
		return (
			<img
				src={media.url}
				alt={name}
				draggable={false}
				className={WHOLE}
				onError={() =>
					mediaFailed(agent, path, translate("Couldn't show {{name}}.", { name: name }))
				}
			/>
		);
	}
	if (media?.state === "failed" || media?.state === "large") {
		return (
			<div
				role="alert"
				className="flex max-w-[420px] flex-col items-center gap-3 rounded-xl border border-media-foreground/10 bg-media/50 px-5 py-4 text-center text-[12.5px] text-media-foreground"
			>
				<Image
					size={22}
					strokeWidth={1.6}
					aria-hidden
					className="text-media-foreground/70"
				/>
				<p>
					{media.state === "failed"
						? media.reason
						: translate("{{name}} is {{bytes}}, too large to show here.", {
								name: name,
								bytes: formatBytes(media.size),
							})}
				</p>
				{media.state === "failed" ? (
					<button
						type="button"
						onClick={() => void loadMedia(source, agent, path, FULL_LIMIT)}
						className={PLAIN_BUTTON}
					>
						<RotateCw size={12} strokeWidth={2} aria-hidden />
						{translate("Try again")}
					</button>
				) : (
					<DownloadButton path={path} name={name} className={PLAIN_BUTTON} />
				)}
			</div>
		);
	}
	const done = media?.state === "loading" ? percent(media.sent, media.total) : null;
	return (
		<div className="relative grid place-items-center">
			{thumbnail ? (
				<img
					src={thumbnail}
					alt=""
					draggable={false}
					className="max-h-[60vh] max-w-[80vw] min-w-[320px] rounded-md object-contain opacity-60 blur-[1px]"
				/>
			) : (
				<div className="h-[240px] w-[360px] rounded-md bg-media/30" />
			)}
			<div
				role="status"
				aria-label={translate("Loading {{name}}", { name: name })}
				className="absolute flex items-center gap-2 rounded-md bg-media/60 px-2.5 py-1.5 text-[12px] text-media-foreground tabular-nums"
			>
				<Spinner size={12} />
				{done !== null ? <span>{done}%</span> : null}
				<button
					type="button"
					onClick={() => cancelMedia(source, agent, path)}
					className="cursor-pointer rounded px-1 text-media-foreground/80 hover:text-media-foreground"
				>
					{translate("Cancel")}
				</button>
			</div>
		</div>
	);
};

/** Where a picture stands in the set the viewer goes through. */
export type PictureNav = {
	readonly at: number;
	readonly count: number;
	/** Moves `delta` pictures along, stopping at the ends. */
	readonly step: (delta: number) => void;
};

const STEP_BUTTON =
	"absolute top-1/2 grid size-[38px] -translate-y-1/2 cursor-pointer place-items-center rounded-full bg-media/50 text-media-foreground hover:bg-media/70 disabled:cursor-default disabled:opacity-30 disabled:hover:bg-media/50";

// The arrow keys step through the set. They're caught before anything else
// hears them, so the composer and the timeline never move under the viewer.
const useStepKeys = (nav: PictureNav | undefined) => {
	const step = nav?.step;
	useEffect(() => {
		if (!step) return;
		const onKey = (event: KeyboardEvent) => {
			if (event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return;
			const delta = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0;
			if (delta === 0) return;
			event.preventDefault();
			event.stopPropagation();
			step(delta);
		};
		window.addEventListener("keydown", onKey, true);
		return () => window.removeEventListener("keydown", onKey, true);
	}, [step]);
};

/** A picture over the window at full size, while `open`. */
export const PictureViewer = ({
	open,
	onOpenChange,
	name,
	picture,
	drag,
	nav,
}: {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	/** The name in its caption. */
	name: string;
	picture: Picture;
	/** Lets the picture drag out as its file. The viewer closes, since it covers the composers. */
	drag?: FileDrag;
	/** Set when the picture is one of a set, to step through the others. */
	nav?: PictureNav;
}) => {
	useI18n();
	useStepKeys(open ? nav : undefined);
	const start = drag?.onDragStart;
	const dragOut: FileDrag = start
		? {
				draggable: true,
				onDragStart: (event) => {
					start(event);
					if (!event.defaultPrevented) setTimeout(() => onOpenChange(false));
				},
			}
		: {};
	return (
		<Dialog.Root open={open} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-media/80 transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				{/* The popup covers the window, so its buttons take focus; a click on its
				    empty part closes it, as one on the backdrop would. */}
				<Dialog.Popup
					onClick={(event) => {
						if (event.target === event.currentTarget) onOpenChange(false);
					}}
					className="fixed inset-0 z-[121] flex flex-col items-center justify-center gap-2 outline-none"
				>
					{open ? (
						<div {...dragOut} className="flex">
							{picture.kind === "url" ? (
								<img
									src={picture.url}
									alt={name}
									draggable={false}
									className={WHOLE}
								/>
							) : (
								<DeviceWhole
									key={picture.path}
									name={name}
									path={picture.path}
									thumbnail={picture.thumbnail ?? null}
								/>
							)}
						</div>
					) : null}
					<div className="flex max-w-[94vw] items-center gap-2 text-[12px] text-media-foreground/80">
						{nav ? (
							<span className="shrink-0 tabular-nums" aria-live="polite">
								{placeLabel(nav.at, nav.count)}
							</span>
						) : null}
						<Dialog.Title className="truncate">{name}</Dialog.Title>
					</div>
					{nav ? (
						<>
							<button
								type="button"
								aria-label={translate("Previous picture")}
								disabled={nav.at === 0}
								onClick={() => nav.step(-1)}
								className={cn(STEP_BUTTON, "left-3")}
							>
								<ChevronLeft aria-hidden size={20} strokeWidth={2} />
							</button>
							<button
								type="button"
								aria-label={translate("Next picture")}
								disabled={nav.at === nav.count - 1}
								onClick={() => nav.step(1)}
								className={cn(STEP_BUTTON, "right-3")}
							>
								<ChevronRight aria-hidden size={20} strokeWidth={2} />
							</button>
						</>
					) : null}
					<Dialog.Close
						aria-label={translate("Close")}
						className="absolute top-3 right-3 grid size-[30px] cursor-pointer place-items-center rounded-[7px] text-media-foreground hover:bg-media-foreground/10"
					>
						<X aria-hidden size={16} strokeWidth={2} />
					</Dialog.Close>
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};

/**
 * A picture as a frame with its name, holding the small preview `maxd` made
 * of it. The webview can't read files on the device, so one with no
 * preview, such as one read back from history, shows an icon instead. A
 * click shows it whole, and for an agent on another device, a button in
 * the corner downloads it. It drags out as its file.
 */
export const PictureThumb = ({ file }: { file: AttachmentRef }) => {
	useI18n();
	const { remote } = useRow();
	const [open, setOpen] = useState(false);
	const drag = useFileDrag(file);
	const picture: Picture = { kind: "device", path: file.path, thumbnail: file.thumbnail };
	const { place, open: openInSet } = useInPictureSet({ name: file.name, picture, drag });
	return (
		<div className="relative">
			<button
				ref={place}
				type="button"
				title={file.name}
				aria-label={translate("Show {{name}} full size", { name: file.name })}
				onClick={openInSet ?? (() => setOpen(true))}
				{...drag}
				className="relative grid h-[94px] w-[176px] cursor-zoom-in place-items-center overflow-hidden rounded-lg border border-input bg-canvas text-subtle-foreground hover:border-ring/60"
			>
				{file.thumbnail ? (
					<img
						src={file.thumbnail}
						alt=""
						draggable={false}
						className="absolute inset-0 size-full object-cover"
					/>
				) : (
					<Image size={20} strokeWidth={1.6} aria-hidden />
				)}
				<span className="absolute inset-x-0 bottom-0 truncate bg-linear-to-t from-media/70 to-transparent px-2 pt-3 pb-1 text-left text-[10.5px] text-media-foreground">
					{file.name}
				</span>
			</button>
			{remote ? (
				<DownloadButton
					path={file.path}
					name={file.name}
					className="absolute top-1 right-1 bg-media/55 text-media-foreground hover:bg-media/70 hover:text-media-foreground"
				/>
			) : null}
			{openInSet ? null : (
				<PictureViewer
					open={open}
					onOpenChange={setOpen}
					name={file.name}
					picture={picture}
					drag={drag}
				/>
			)}
		</div>
	);
};
