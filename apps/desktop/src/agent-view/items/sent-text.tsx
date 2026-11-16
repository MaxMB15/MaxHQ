// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The words of a message you sent, as Markdown, with each pill where you put
// it. The pills read only, and a click does what it does in the composer: a
// picture shows whole, an agent or a note opens its pane, a quote goes back
// to its passage and a pull request or issue opens on GitHub. A pill whose source is
// gone keeps its label and shows as unresolved. A copy gives each pill's plain
// form, the way another app can read it, and carries the pills too, so a
// paste in a composer gets them back.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useRow } from "@maxhq/shared/agent-view/context";
import type { AttachmentId, AttachmentRef, InlineRef, Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { encodeFragment } from "@maxhq/shared/pills/clipboard";
import type { AgentRef, QuoteRef } from "@maxhq/shared/pills/draft";
import {
	pieceText,
	piecesSource,
	type SentFacts,
	sentFragment,
	type SentPiece,
} from "@maxhq/shared/pills/sent";
import { deviceName } from "@maxhq/shared/view/select";
import { type ClipboardEvent, useMemo, useState } from "react";

import { jumpToQuote } from "@/agent-view/jump";
import { Markdown } from "@/agent-view/markdown";
import { BLOCK_SELECTOR } from "@/agent-view/selection";
import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { writeDraftClipboard } from "@/composer/pill-keys";
import { NotePill } from "@/notes/pill";
import { type FileRecord, presentPill } from "@/pills/registry";
import { ShownPill } from "@/pills/shown";
import { ReadVisualPage } from "@/visuals/visual-preview";

import { attachmentMeta } from "@maxhq/shared/agent-view/format";
import { PictureViewer } from "../picture-viewer";

/** How often a pill's preview updates its times, such as when a pull request was checked. */
const PILL_CLOCK_MS = 30_000;

const READY = { state: "ready" } as const;

const MISSING: FileRecord = {
	get name() {
		return t("Removed file");
	},
	kind: "missing",
	bytes: null,
	mime: null,
	path: null,
	image: null,
	fullImage: false,
	number: undefined,
	progress: READY,
};

/** The props a pill takes when a click on it does something. */
const action = (run: () => void) => ({ className: "pill-action", tabIndex: 0, onClick: run });

/**
 * Marks a pill's element with its piece, so a copy can find the piece, and
 * leaves it out of what find counts.
 */
const marked = (index: number) => ({ "data-piece": index, "data-find-skip": "" });

const FilePill = ({
	file,
	number,
	index,
	now,
}: {
	file: AttachmentRef | undefined;
	number: number | undefined;
	index: number;
	now: Timestamp;
}) => {
	useI18n();
	const [open, setOpen] = useState(false);
	const record: FileRecord = file
		? {
				name: file.name,
				kind: file.kind,
				bytes: file.bytes,
				mime: file.mime,
				path: file.path,
				image: file.thumbnail,
				fullImage: false,
				number,
				progress: READY,
				detail: attachmentMeta(file),
			}
		: MISSING;
	const presentation = presentPill({ kind: "attachment", file: record }, { now });
	if (file?.kind !== "image") return <ShownPill presentation={presentation} {...marked(index)} />;
	return (
		<>
			<ShownPill
				presentation={presentation}
				{...marked(index)}
				{...action(() => setOpen(true))}
			/>
			<PictureViewer
				open={open}
				onOpenChange={setOpen}
				name={file.name}
				picture={{ kind: "device", path: file.path, thumbnail: file.thumbnail }}
			/>
		</>
	);
};

const QuotePill = ({ quote, index, now }: { quote: QuoteRef; index: number; now: Timestamp }) => {
	const gone = useView((state) => !state.view.tables.agent.has(quote.agent));
	const presentation = presentPill({ kind: "quote", ref: quote, gone }, { now });
	return (
		<ShownPill
			presentation={presentation}
			{...marked(index)}
			{...(gone ? {} : action(() => jumpToQuote(quote)))}
		/>
	);
};

const AgentPill = ({ agent, index, now }: { agent: AgentRef; index: number; now: Timestamp }) => {
	const services = useServices();
	const found = useView((state) => state.view.tables.agent.get(agent.agent) ?? null);
	const device = useView((state) => deviceName(state.view, agent.device));
	const presentation = presentPill(
		{ kind: "agent", ref: agent, agent: found, deviceName: device },
		{ now }
	);
	return (
		<ShownPill
			presentation={presentation}
			{...marked(index)}
			{...(found ? action(() => openAgent(services, agent.agent)) : {})}
		/>
	);
};

/** A visual you sent, pink like its composer pill, its page read once the preview opens. */
const SentVisualPill = ({
	visual,
	file,
	index,
	now,
}: {
	visual: Extract<InlineRef, { type: "visual" }>;
	file: AttachmentRef | undefined;
	index: number;
	now: Timestamp;
}) => {
	const { t } = useI18n();
	const { agent } = useRow();
	const presentation = presentPill(
		{
			kind: "visual",
			title: visual.title,
			from: visual.from ?? null,
			bytes: file?.bytes ?? null,
			progress: file ? READY : { state: "failed", error: t("The page is gone") },
			page: file ? <ReadVisualPage agent={agent} file={file} title={visual.title} /> : null,
		},
		{ now }
	);
	return <ShownPill presentation={presentation} {...marked(index)} />;
};

const PieceView = ({
	piece,
	index,
	files,
	numbers,
	now,
}: {
	piece: Exclude<SentPiece, { type: "text" }>;
	/** Where the piece is in the message. */
	index: number;
	files: ReadonlyMap<AttachmentId, AttachmentRef>;
	numbers: ReadonlyMap<AttachmentId, number>;
	now: Timestamp;
}) => {
	if (piece.type === "git")
		return (
			<ShownPill
				presentation={presentPill({ kind: "git_item", ref: piece.ref }, { now })}
				{...marked(index)}
				className="pill-action"
				role="link"
				tabIndex={0}
				render={<a href={piece.ref.url} target="_blank" rel="noreferrer" />}
			/>
		);
	const { ref } = piece;
	switch (ref.type) {
		case "attachment":
			return (
				<FilePill
					file={files.get(ref.id)}
					number={numbers.get(ref.id)}
					index={index}
					now={now}
				/>
			);
		case "quote":
			return <QuotePill quote={ref} index={index} now={now} />;
		case "agent":
			return <AgentPill agent={ref} index={index} now={now} />;
		case "note":
			return <NotePill note={ref} now={now} tabIndex={0} {...marked(index)} />;
		case "visual":
			return (
				<SentVisualPill
					visual={ref}
					file={files.get(ref.attachment)}
					index={index}
					now={now}
				/>
			);
		case "output":
			return (
				<ShownPill
					presentation={presentPill({ kind: "output", ref }, { now })}
					{...marked(index)}
				/>
			);
	}
};

const LINE_BREAK: SentPiece = { type: "text", text: "\n" };

/**
 * The pieces in a copied node: its text as far as the selection holds it,
 * and each pill in it whole, even one the selection only touches.
 */
const piecesIn = (node: Node, pieces: readonly SentPiece[]): SentPiece[] => {
	// A line break alone in a text node only lays out the rendered Markdown.
	if (node.nodeType === Node.TEXT_NODE)
		return node.nodeValue === "\n" ? [] : [{ type: "text", text: node.nodeValue ?? "" }];
	if (node instanceof Element && node.hasAttribute("data-piece")) {
		const piece = pieces[Number(node.getAttribute("data-piece"))];
		return piece ? [piece] : [];
	}
	if (node instanceof Element && node.matches("[data-find-skip]")) return [];
	if (node instanceof Element && node.tagName === "BR") return [LINE_BREAK];
	const inner = Array.from(node.childNodes, (child) => piecesIn(child, pieces)).flat();
	return node instanceof Element && node.matches(BLOCK_SELECTOR) ? [...inner, LINE_BREAK] : inner;
};

/**
 * The part of a selected range inside `root`, or null when the range holds
 * text outside it too. A triple click selects up to the next line, which
 * holds none.
 */
const within = (root: Element, range: Range): Range | null => {
	if (!range.intersectsNode(root)) return null;
	const clipped = range.cloneRange();
	if (!root.contains(clipped.startContainer)) clipped.setStart(root, 0);
	if (!root.contains(clipped.endContainer)) clipped.setEnd(root, root.childNodes.length);
	return clipped.toString() === range.toString() ? clipped : null;
};

/**
 * The pieces of the message the selection in `root` holds. Null leaves the
 * copy as it is, when the selection holds no pill or runs past the message.
 */
const copiedPieces = (root: Element, pieces: readonly SentPiece[]): SentPiece[] | null => {
	const selection = root.ownerDocument.getSelection();
	if (!selection || selection.isCollapsed) return null;
	const copied: SentPiece[] = [];
	for (let index = 0; index < selection.rangeCount; index++) {
		const range = within(root, selection.getRangeAt(index));
		if (!range) return null;
		copied.push(...piecesIn(range.cloneContents(), pieces));
	}
	if (copied.at(-1) === LINE_BREAK) copied.pop();
	return copied.some((piece) => piece.type !== "text") ? copied : null;
};

/** A sent message's text with its pills. */
export const SentText = ({
	pieces,
	files,
	factsOf,
	numbers,
}: {
	pieces: readonly SentPiece[];
	files: ReadonlyMap<AttachmentId, AttachmentRef>;
	factsOf: SentFacts;
	numbers: ReadonlyMap<AttachmentId, number>;
}) => {
	const now = useNow(PILL_CLOCK_MS);
	const { agent } = useRow();
	const services = useServices();
	const onCopy = (event: ClipboardEvent<HTMLElement>) => {
		const copied = copiedPieces(event.currentTarget, pieces);
		if (copied === null) return;
		event.preventDefault();
		// The device is read as the copy happens, so an agent that moved names where it is.
		const { view } = services.view.getState();
		const record = view.tables.agent.get(agent);
		const device = (record && view.tables.workspace.get(record.workspace)?.device) ?? null;
		const fragment = sentFragment(copied, (id) => files.get(id), { agent, device });
		writeDraftClipboard(
			event.clipboardData,
			copied.map((piece) => pieceText(piece, factsOf, numbers)).join(""),
			encodeFragment(fragment)
		);
	};
	const source = useMemo(() => piecesSource(pieces), [pieces]);
	const plain = useMemo(
		() => (index: number) => {
			const piece = pieces[index];
			return piece && piece.type !== "text" ? pieceText(piece, factsOf, numbers) : null;
		},
		[pieces, factsOf, numbers]
	);
	const draw = (index: number) => {
		const piece = pieces[index];
		return piece && piece.type !== "text" ? (
			<PieceView piece={piece} index={index} files={files} numbers={numbers} now={now} />
		) : null;
	};
	return (
		<div className="min-w-0" onCopy={onCopy}>
			<Markdown text={source} className="leading-normal" written={{ draw, plain }} />
		</div>
	);
};
