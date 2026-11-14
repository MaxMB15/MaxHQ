// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What you sent, on the right in a bubble, the files the agent sent you,
// on the left in a frame, and the agent's reply as Markdown across the
// column, with a button under it that copies the reply. A picture in
// either opens whole on a click. A file you placed in your message's text
// carries the number its pill shows, and an agent's excerpt goes to the
// agent without showing as a file.

import { File, FileText, Folder, Image } from "lucide-react";
import { useMemo } from "react";
import { useStore } from "zustand";

import type { AttachmentId, AttachmentRef } from "@maxhq/shared/bindings";
import { sentMessage } from "@maxhq/shared/pills/sent";
import { useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";
import { type VisualOrigin, VisualOriginContext } from "@/visuals/origin";
import { formatClock } from "@maxhq/shared/status/format";
import { visualVersions } from "@maxhq/shared/visuals/versions";

import { useRow } from "@maxhq/shared/agent-view/context";
import { DownloadButton } from "../download";
import { useFileDrag } from "../file-drag";
import { attachmentMeta, formatBytes, sentFilesLine } from "@maxhq/shared/agent-view/format";
import { Markdown } from "../markdown";
import { PictureSet } from "../picture-set";
import { PictureThumb } from "../picture-viewer";
import { SentText } from "./sent-text";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const FILE_ICON = { pdf: FileText, file: File, folder: Folder, image: Image } as const;

// What a file is, with its size, and first the number its pill shows,
// such as "PDF 1, 412 KB".
const metaOf = (file: AttachmentRef, number: number | undefined): string =>
	number === undefined
		? attachmentMeta(file)
		: `${file.kind === "pdf" ? "PDF" : translate("Image")} ${number}, ${formatBytes(file.bytes)}`;

const FileChip = ({
	file,
	remote,
	number,
}: {
	file: AttachmentRef;
	remote: boolean;
	number?: number;
}) => {
	useI18n();
	const Icon = FILE_ICON[file.kind];
	const drag = useFileDrag(file);
	return (
		<div
			{...drag}
			title={file.path}
			className={cn(
				"flex h-10 max-w-[260px] min-w-0 items-center gap-2 rounded-lg border border-input bg-card pl-2",
				remote ? "pr-1.5" : "pr-3"
			)}
		>
			<span className="grid size-[26px] shrink-0 place-items-center rounded-[5px] bg-border text-secondary-foreground">
				<Icon size={14} strokeWidth={1.8} aria-hidden />
			</span>
			<span className="flex min-w-0 flex-col">
				<span className="truncate text-[12px] leading-tight text-foreground">
					{file.name}
				</span>
				<span className="truncate text-[11px] leading-tight text-muted-foreground">
					{metaOf(file, number)}
				</span>
			</span>
			{remote ? <DownloadButton path={file.path} name={file.name} /> : null}
		</div>
	);
};

// A picture with the number its pill shows, such as "Image 1", in a corner.
const NumberedThumb = ({ file, number }: { file: AttachmentRef; number: number | undefined }) => {
	useI18n();
	return number === undefined ? (
		<PictureThumb file={file} />
	) : (
		<div className="relative">
			<PictureThumb file={file} />
			<span className="pointer-events-none absolute top-1 left-1 rounded-[4px] bg-media/60 px-1.5 py-px text-[10.5px] font-medium text-media-foreground select-none">
				{translate("Image ")}
				{number}
			</span>
		</div>
	);
};

// The pictures in a row above the other files.
const Attachments = ({
	files,
	numbers,
}: {
	files: readonly AttachmentRef[];
	/** The number each placed image and PDF shows on its pill. */
	numbers?: ReadonlyMap<AttachmentId, number>;
}) => {
	useI18n();
	const { remote } = useRow();
	const images = files.filter((a) => a.kind === "image");
	const others = files.filter((a) => a.kind !== "image");
	return (
		<>
			{images.length > 0 ? (
				<PictureSet>
					<div className="flex flex-wrap gap-2">
						{images.map((file) => (
							<NumberedThumb
								key={file.id}
								file={file}
								number={numbers?.get(file.id)}
							/>
						))}
					</div>
				</PictureSet>
			) : null}
			{others.length > 0 ? (
				<div className="flex flex-wrap gap-2">
					{others.map((file) => (
						<FileChip
							key={file.id}
							file={file}
							remote={remote}
							number={numbers?.get(file.id)}
						/>
					))}
				</div>
			) : null}
		</>
	);
};

/** A message you sent. */
export const UserMessage = ({ item }: { item: ItemOf<"user_message"> }) => {
	useI18n();
	const at = item.started_at ?? item.completed_at;
	const sent = useMemo(() => sentMessage(item), [item]);
	return (
		<div className="flex flex-col items-end gap-1">
			<div className="flex max-w-[78%] min-w-0 flex-col gap-[9px] rounded-xl border border-track bg-secondary px-3 py-2.5">
				<Attachments files={sent.shown} numbers={sent.numbers} />
				{item.text ? (
					<SentText
						pieces={sent.pieces}
						files={sent.files}
						factsOf={sent.factsOf}
						numbers={sent.numbers}
					/>
				) : null}
			</div>
			{at !== null ? (
				<span className="pr-1 text-[11px] text-muted-foreground tabular-nums">
					{item.delivery === "steer"
						? translate("{{clock}}, sent while it worked", { clock: formatClock(at) })
						: formatClock(at)}
				</span>
			) : null}
		</div>
	);
};

/** Files the agent sent you, with the message it sent with them. */
export const AgentAttachments = ({ item }: { item: ItemOf<"agent_attachments"> }) => {
	useI18n();
	const at = item.completed_at ?? item.started_at;
	const sent = sentFilesLine(item.attachments.length);
	return (
		<div className="flex flex-col items-start gap-1">
			<div className="flex max-w-[78%] min-w-0 flex-col gap-[9px] rounded-xl border border-track bg-card px-3 py-2.5">
				<Attachments files={item.attachments} />
				{item.text ? <Markdown text={item.text} className="gap-2" /> : null}
			</div>
			<span className="pl-1 text-[11px] text-muted-foreground tabular-nums">
				{at !== null ? `${sent}, ${formatClock(at)}` : sent}
			</span>
		</div>
	);
};

// A blinking bar after the last block while the reply streams in.
const CARET =
	"[&>:last-child]:after:ml-0.5 [&>:last-child]:after:text-muted-foreground [&>:last-child]:after:content-['▍'] motion-safe:[&>:last-child]:after:animate-pulse";

// The row under a finished reply. It shows while the pointer is on the
// reply or the button has focus, and always on a touch screen.
const REPLY_ACTIONS =
	"flex h-[22px] items-center opacity-0 transition-opacity duration-150 select-none group-hover/reply:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100";

const COPY_LOOK =
	"-ml-[7px] inline-flex h-[22px] items-center gap-1.5 rounded-[5px] px-[7px] text-[11.5px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

/**
 * Where the reply's visuals came from, so Tell the agent can quote their
 * errors back. An agent you only watch has no composer to quote into.
 */
const useOrigin = (item: ItemOf<"assistant_message">): VisualOrigin | null => {
	const { agent } = useRow();
	const found = useView((s) => s.view.tables.agent.get(agent));
	const title = found && found.control !== "observed" ? found.title : null;
	return useMemo(
		() => (title === null ? null : { agent: { id: agent, title }, item }),
		[agent, title, item]
	);
};

/** The agent's reply, and once it has finished, a button that copies its Markdown. */
export const AssistantMessage = ({ item }: { item: ItemOf<"assistant_message"> }) => {
	useI18n();
	const origin = useOrigin(item);
	const { timeline } = useRow();
	// A reply with no words in a turn that showed a visual says nothing, so
	// it doesn't show at all.
	const blank = item.text.trim() === "";
	const hidden = useStore(
		timeline,
		(state) =>
			blank && item.turn !== null && visualVersions(state.items).turnHasVisual(item.turn)
	);
	if (hidden) return null;
	if (item.text === "" && item.streaming) {
		return (
			<p className="text-[13px] text-muted-foreground motion-safe:animate-pulse">
				{translate("Writing")}
			</p>
		);
	}
	// The reply keeps its place in the tree when it finishes, so it doesn't draw again.
	return (
		<div className="group/reply flex flex-col gap-1">
			<VisualOriginContext value={origin}>
				<Markdown
					text={item.text}
					streaming={item.streaming}
					className={item.streaming ? `mt-0.5 gap-2.5 ${CARET}` : "mt-0.5 gap-2.5"}
				/>
			</VisualOriginContext>
			{!item.streaming && item.text.trim() !== "" ? (
				<div className={REPLY_ACTIONS}>
					<CopyButton
						text={item.text}
						title={translate("Copy the reply as Markdown")}
						className={COPY_LOOK}
					/>
				</div>
			) : null}
		</div>
	);
};
