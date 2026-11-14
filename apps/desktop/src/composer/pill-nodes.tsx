// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pills that take a place in the text: a file, a visual, a quote, an
// agent and a note.
// Each is one atom that reads as one object mark, so the draft's offsets,
// spell check and the sent text all count it as one character. The nodes
// keep only what names the pill. A file's state lives in the composer's
// attachments, which each pill reads through context.

import type { Agent, AgentId, DeviceId, Timestamp } from "@maxhq/shared/bindings";
import {
	type AgentRef,
	type NoteRef,
	OBJECT_MARK,
	type QuoteRef,
	type VisualPill as VisualPillRef,
} from "@maxhq/shared/pills/draft";
import { visualBytes } from "@maxhq/shared/visuals/visual";
import type { FileProgress } from "@maxhq/shared/pills/label";
import { Extension, mergeAttributes, Node } from "@tiptap/core";
import { NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from "@tiptap/react";
import { createContext, type ComponentType, use } from "react";
import { useStore } from "zustand";

import { jumpToQuote } from "@/agent-view/jump";
import { NotePill as NotePillShown } from "@/notes/pill";
import { type FileRecord, presentPill } from "@/pills/registry";
import { ShownPill } from "@/pills/shown";
import { ReadVisualPage, ScaledVisual } from "@/visuals/visual-preview";

import { type AttachmentsStore, type Chip, chipDetail } from "./attachments";
import { PILL_NAMES, PILL_NODES } from "./doc";
import type { ComposerNative } from "./native";
import { useThumbnail } from "./thumbnail";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** What the pills in one composer read. */
export type PillsContextValue = {
	readonly attachments: AttachmentsStore;
	readonly native: ComposerNative;
	/** Each image's and PDF's number, by the file's key. */
	readonly numbers: ReadonlyMap<string, number>;
	readonly now: Timestamp;
	readonly agentOf: (id: string) => Agent | null;
	readonly deviceName: (id: DeviceId) => string | null;
	/** The composer's agent, which reads a staged visual's page when the composer doesn't hold it. */
	readonly agent?: AgentId;
};

export const PillsContext = createContext<PillsContextValue | null>(null);

/** How long the agents' pills wait to say how long ago each agent was active again. */
export const PILL_CLOCK_MS = 30_000;

const usePills = (): PillsContextValue => {
	const value = use(PillsContext);
	if (!value) throw new Error("A pill needs its composer's context");
	return value;
};

/** How far a file has got, the way its pill says it. */
export const progressOf = (chip: Chip): FileProgress => {
	switch (chip.state) {
		case "staging":
			return { state: "staging" };
		case "uploading": {
			const upload = chip.upload;
			const percent =
				upload && upload.total > 0
					? Math.min(100, Math.round((upload.sent / upload.total) * 100))
					: null;
			return { state: "uploading", percent };
		}
		case "failed":
			return { state: "failed", error: chip.error ?? translate("Couldn't get it ready") };
		case "staged":
			return { state: "ready" };
	}
};

const MISSING: FileRecord = {
	get name() {
		return translate("Removed file");
	},
	kind: "missing",
	bytes: null,
	mime: null,
	path: null,
	image: null,
	fullImage: false,
	number: undefined,
	progress: { state: "ready" },
};

/** A thumbnail big enough for the preview too. */
const THUMBNAIL_SIZE = 320;

const ACTION =
	"h-6 rounded-[5px] border border-input bg-secondary px-2 text-[11.5px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring";

/** Keeps the caret in the text box when a button in a pill's preview is pressed. */
const keepFocus = (event: { preventDefault: () => void }) => event.preventDefault();

const FilePillView = ({ node, selected, deleteNode }: ReactNodeViewProps) => {
	useI18n();
	const { attachments, native, numbers, now } = usePills();
	const key = String(node.attrs.key);
	const chip = useStore(attachments, (state) => state.chips.find((each) => each.key === key));
	const thumbnail = useThumbnail(
		native,
		chip?.kind === "image" && chip.preview === null ? chip.source : null,
		THUMBNAIL_SIZE
	);
	const file: FileRecord = chip
		? {
				name: chip.name,
				kind: chip.kind,
				bytes: chip.bytes,
				mime: chip.mime,
				path: chip.source ?? chip.ref?.path ?? null,
				image: chip.preview ?? thumbnail ?? chip.ref?.thumbnail ?? null,
				fullImage: chip.preview !== null,
				number: numbers.get(key),
				progress: progressOf(chip),
				...(chip.state === "failed" ? {} : { detail: chipDetail(chip) }),
			}
		: MISSING;
	const presentation = presentPill({ kind: "attachment", file }, { now });
	const footer = chip?.state === "failed" && (
		<div className="mt-2 flex gap-1.5">
			{chip.retryable && (
				<button
					type="button"
					className={ACTION}
					onMouseDown={keepFocus}
					onClick={() => attachments.getState().retry(key)}
				>
					{translate("Retry")}
				</button>
			)}
			<button
				type="button"
				className={ACTION}
				onMouseDown={keepFocus}
				onClick={() => deleteNode()}
			>
				{translate("Remove")}
			</button>
		</div>
	);
	return (
		<NodeViewWrapper as="span" className="pill-node" contentEditable={false}>
			<ShownPill presentation={presentation} selected={selected} footer={footer || null} />
		</NodeViewWrapper>
	);
};

const VisualPillView = ({ node, selected, deleteNode }: ReactNodeViewProps) => {
	useI18n();
	const { attachments, agent, now } = usePills();
	const ref = node.attrs.ref as VisualPillRef;
	const chip = useStore(attachments, (state) => state.chips.find((each) => each.key === ref.key));
	const page =
		chip?.page !== undefined ? (
			<ScaledVisual html={chip.page} title={ref.title} />
		) : chip?.ref && agent !== undefined ? (
			// A page the composer doesn't hold, such as one from a message
			// brought back to edit, is read once the preview opens.
			<ReadVisualPage agent={agent} file={chip.ref} title={ref.title} />
		) : null;
	const presentation = presentPill(
		{
			kind: "visual",
			title: ref.title,
			from: ref.from,
			bytes: chip?.page !== undefined ? visualBytes(chip.page) : (chip?.bytes ?? null),
			progress: chip
				? progressOf(chip)
				: { state: "failed", error: translate("The page is gone") },
			page,
		},
		{ now }
	);
	const footer = chip?.state === "failed" && (
		<div className="mt-2 flex gap-1.5">
			{chip.retryable && (
				<button
					type="button"
					className={ACTION}
					onMouseDown={keepFocus}
					onClick={() => attachments.getState().retry(ref.key)}
				>
					{translate("Retry")}
				</button>
			)}
			<button
				type="button"
				className={ACTION}
				onMouseDown={keepFocus}
				onClick={() => deleteNode()}
			>
				{translate("Remove")}
			</button>
		</div>
	);
	return (
		<NodeViewWrapper as="span" className="pill-node" contentEditable={false}>
			<ShownPill presentation={presentation} selected={selected} footer={footer || null} />
		</NodeViewWrapper>
	);
};

const QuotePillView = ({ node, selected }: ReactNodeViewProps) => {
	useI18n();
	const { now } = usePills();
	const ref = node.attrs.ref as QuoteRef;
	return (
		<NodeViewWrapper as="span" className="pill-node" contentEditable={false}>
			<ShownPill
				presentation={presentPill(
					{ kind: "quote", ref, onJump: () => jumpToQuote(ref) },
					{ now }
				)}
				selected={selected}
			/>
		</NodeViewWrapper>
	);
};

const AgentPillView = ({ node, selected }: ReactNodeViewProps) => {
	useI18n();
	const { now, agentOf, deviceName } = usePills();
	const ref = node.attrs.ref as AgentRef;
	const presentation = presentPill(
		{ kind: "agent", ref, agent: agentOf(ref.agent), deviceName: deviceName(ref.device) },
		{ now }
	);
	return (
		<NodeViewWrapper as="span" className="pill-node" contentEditable={false}>
			<ShownPill presentation={presentation} selected={selected} />
		</NodeViewWrapper>
	);
};

const NotePillView = ({ node, selected }: ReactNodeViewProps) => {
	useI18n();
	const { now } = usePills();
	return (
		<NodeViewWrapper as="span" className="pill-node" contentEditable={false}>
			<NotePillShown note={node.attrs.ref as NoteRef} now={now} selected={selected} />
		</NodeViewWrapper>
	);
};

const pillNode = (
	name: string,
	attribute: "key" | "ref",
	view: ComponentType<ReactNodeViewProps>
) =>
	Node.create({
		name,
		group: "inline",
		inline: true,
		atom: true,
		selectable: true,
		draggable: false,
		addAttributes: () => ({
			[attribute]: {
				default: null,
				rendered: false,
			},
		}),
		parseHTML: () => [{ tag: `span[data-pill="${name}"]` }],
		renderHTML: ({ HTMLAttributes }) => [
			"span",
			mergeAttributes({ "data-pill": name }, HTMLAttributes),
		],
		renderText: () => OBJECT_MARK,
		addNodeView: () => ReactNodeViewRenderer(view, { as: "span", className: "pill-host" }),
	});

export const FilePill = pillNode(PILL_NODES.attachment, "key", FilePillView);
export const VisualPill = pillNode(PILL_NODES.visual, "ref", VisualPillView);
export const QuotePill = pillNode(PILL_NODES.quote, "ref", QuotePillView);
export const AgentPill = pillNode(PILL_NODES.agent, "ref", AgentPillView);
export const NotePill = pillNode(PILL_NODES.note, "ref", NotePillView);

/**
 * Makes each pill read as one object mark wherever ProseMirror turns the
 * document into text, such as a paragraph's text and the text between two
 * positions.
 */
export const PillText = Extension.create({
	name: "pillText",
	extendNodeSchema: (extension) =>
		PILL_NAMES.has(extension.name) ? { leafText: () => OBJECT_MARK } : {},
});

/** Every extension the pills need. */
export const PILL_EXTENSIONS = [FilePill, VisualPill, QuotePill, AgentPill, NotePill, PillText];
