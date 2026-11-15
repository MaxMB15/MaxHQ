// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one input for every agent, Claude or Codex, wherever it runs. It
// sends at once while the agent is idle, queues or steers while a turn
// runs, and queues while a prompt waits. Files, quotes, other agents and
// notes go into the text as pills: files by drop, paste, the picker, the
// `@` menu and a drag out of a timeline, agents and notes by the `@` menu
// or a drag from the sidebar. A file dropped or pasted on an answer field
// in the pane comes here as if dropped here, and the focus stays put. An
// agent on another device goes with an excerpt taken when you send, and a
// note from another project with a copy made then. A `#123` stays text
// and shows as a pill once it resolves, and one picked a while ago is
// looked up again when you send.
// A pasted link to a
// pull request or issue goes in as text and becomes its `#123`, or
// `owner/repo#123` for another repo, once the lookup finds it. An agent
// you started in your own terminal gets no composer. On a remote device,
// files upload as they arrive and the message waits for them. The draft
// and its files stay with the agent while the app runs, so closing its pane
// and opening it again keeps what you typed, and this Mac saves the draft
// too, so it comes back after a restart.

import { keep as keepDraft, keptValue, useKept } from "@maxhq/shared/agent-view/kept";
import { useNow } from "@maxhq/shared/lib/clock";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import {
	type HistoryCaret,
	type HistoryDirection,
	historyEntries,
	type HistoryPosition,
	stepHistory,
} from "@maxhq/shared/composer/history";
import type {
	Agent,
	AgentId,
	AttachmentId,
	DeviceId,
	NoteId,
	SendMode,
	TimelineItem,
	UploadTarget,
} from "@maxhq/shared/bindings";
import {
	type ClipboardFile,
	type ClipboardFragment,
	encodeFragment,
	readFragment,
} from "@maxhq/shared/pills/clipboard";
import {
	type AgentRef,
	excerptIds,
	farAgents,
	farNotes,
	type FileFacts,
	fileKeys,
	fileMarker,
	fileNumbers,
	fromUserInput,
	type GitItemRef,
	gitKey,
	gitNumbers,
	type GitTarget,
	gitTargetOf,
	isBlank,
	keyedGitItems,
	markedText,
	type NoteRef,
	plainText,
	type Segment,
	toUserInput,
	trimEnd,
	visualMarker,
	withExcerpts,
} from "@maxhq/shared/pills/draft";
import { type GitLink, gitLinkOf, sameRepo } from "@maxhq/shared/pills/git-link";
import { refreshGitItems } from "@maxhq/shared/pills/git-refresh";
import { agentPillOf } from "@maxhq/shared/pills/agent-pill";
import { messageFragment } from "@maxhq/shared/pills/sent";
import type { TriggerKind } from "@maxhq/shared/pills/trigger";
import type { TimelineStore } from "@maxhq/shared/timeline/store";
import { VENDOR_NAME } from "@maxhq/shared/status/usage";
import { errorText } from "@maxhq/shared/timeline/store";
import { draggedName } from "@maxhq/shared/ui/rows";
import { noteCopyName, notePillBlocked, notesReach, shownTitle } from "@maxhq/shared/view/notes";
import { deviceName, linkForDevice, modelsOf, movesOf } from "@maxhq/shared/view/select";
import { canSetEffort } from "@maxhq/shared/agent-view/effort";
import { runningMode } from "@maxhq/shared/agent-view/controls";
import { sendsVisuals } from "@maxhq/shared/view/visuals";
import { fitsVisual, readVisual, visualFragment } from "@maxhq/shared/pills/visual";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import {
	VISUAL_MIME,
	type VisualTransfer,
	writeVisualTransfer,
} from "@maxhq/shared/visuals/transfer";
import { Clock, StickyNote, TextQuote, Upload } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import type { DraggedFile } from "@/agent-view/file-drag";
import { VendorIcon } from "@/agent-view/icons";
import { askForRequest } from "@/agent-view/request";
import { useServices, useView } from "@/app/services";
import { agreedSend, pickMode } from "@/settings/auto-consent";

import { agentDrops, draggedAgentsOf, draggedNoteOf } from "./agent-drop";
import {
	anyStaging,
	anyUploading,
	type AttachmentsStore,
	type Chip,
	chipCount,
	chipSummary,
	chipsOf,
	createAttachments,
	type StageContext,
	stagedIds,
} from "./attachments";
import "./composer.css";
import { type EditorChange, PromptEditor, type PromptEditorHandle } from "./editor";
import {
	attachFailure,
	copyFailure,
	excerptFailure,
	type ExcerptFailure,
	intakeFailure,
} from "./failure";
import { isOver, timelineFileOf } from "./file-drop";
import {
	createGitPills,
	gitFinder,
	GitPillPreview,
	type GitPillsStore,
	shownItems,
	useGitHover,
} from "./git-pills";
import { ImageNote } from "./image-note";
import { useComposerNative } from "./native";
import { type PaneFile, usePaneFiles } from "./pane-files";
import { nextPastedTextFileName, pastesAsFile } from "./paste";
import { EffortPicker } from "./effort-picker";
import { type Attached, AttachButton, FooterRow, SendButtons, type Sends } from "./footer";
import { ModelPicker, PermissionPicker } from "./pickers";
import { usePillMenu } from "./pill-menu";
import { PILL_CLOCK_MS, PillsContext, type PillsContextValue } from "./pill-nodes";
import { QueuedList } from "./queued";
import { quoteOfDrag, visualOfDrag } from "./quote-drop";
import { quoteInbox } from "./quote-inbox";
import { releasedFiles } from "./retained";
import { draftsOf } from "./saved-drafts";
import { useStopKey } from "./stop-key";
import {
	attempt,
	canStop,
	type ComposerAction,
	failureOf,
	nowMode,
	placeholderFor,
	type QueuedMessage,
	turnOf,
	waitingMessages,
} from "@maxhq/shared/composer/send";
import { UploadStrip } from "./upload-strip";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** What the agent view hands the composer. */
export type ComposerProps = {
	/** The agent the composer sends to. */
	readonly agent: AgentId;
	/** The device the agent runs on, where attachments stage. */
	readonly device: DeviceId;
	/** The agent's messages that wait for the turn to end. Anything else in it is skipped. */
	readonly queued: readonly TimelineItem[];
	/** The agent's timeline, where Up and Down find the messages you sent. */
	readonly timeline: TimelineStore;
	/**
	 * What takes file and agent drops, such as the agent's whole pane. Null
	 * waits for it to mount. Left out, the composer's own box takes them.
	 * ⌘. stops the turn while focus is anywhere in it.
	 */
	readonly dropTarget?: Element | null;
};

/** The first free name among `pasted-image.png`, `pasted-image-2.png` and on. */
const nextPastedImageName = (existing: readonly string[]): string => {
	const names = new Set(existing.map((name) => name.toLowerCase()));
	if (!names.has("pasted-image.png")) return "pasted-image.png";
	for (let sequence = 2; ; sequence += 1) {
		const candidate = `pasted-image-${sequence}.png`;
		if (!names.has(candidate)) return candidate;
	}
};

/** Puts pills in the box, at the caret or where a drop goes. */
type Put = (segments: Segment[]) => void;

/** A pill for each file, with a space between them. */
const filePills = (keys: readonly string[]): Segment[] =>
	keys.flatMap((key, index): Segment[] => [
		...(index > 0 ? [{ type: "text", text: " " } as const] : []),
		{ type: "pill", pill: { type: "attachment", key } },
	]);

/** A message the queue gave back, as the draft model reads one. */
const inputOf = (message: QueuedMessage) => ({
	text: message.text,
	attachments: message.attachments.map((ref) => ref.id),
	...(message.refs && message.refs.length > 0 ? { refs: message.refs } : {}),
});

const EMPTY_CHANGE: EditorChange = { text: "", segments: [], caret: 0, held: new Set() };

/** Where an agent's draft is kept, and the files its pills stand for. */
const draftKey = (agent: AgentId) => `draft:${agent}`;
const draftFilesKey = (agent: AgentId) => `draft-files:${agent}`;

/** Where a walk through an agent's earlier messages is kept, with the draft it put aside. */
const walkKey = (agent: AgentId) => `history:${agent}`;

/** Every word that opens a menu, as the composer's text box has them all. */
const ALL_TRIGGERS: readonly TriggerKind[] = ["path", "git-item", "skill"];

/** How much of an agent on another device its excerpt holds, in characters. */
const EXCERPT_BUDGET = 40_000;

/** An agent's excerpt a send took: its chip, or why it couldn't be taken. */
type Excerpt = { readonly key: string } | { readonly error: string | null };

/** Whether the text holds a note's pill. */
const hasNote = (segments: readonly Segment[]): boolean =>
	segments.some((segment) => segment.type === "pill" && segment.pill.type === "note");

/** Where a piece of a draft goes: a composer's files and pills, and where its files stage. */
type Placing = {
	readonly attachments: AttachmentsStore;
	readonly gitPills: GitPillsStore;
	readonly context: StageContext;
	/** The composer's name in the drafts it copies. */
	readonly origin: string;
};

/**
 * A piece of a draft with its pills, ready to go in the box. A file from
 * this composer keeps its place in the attachments, and one an agent
 * has, such as a file in a sent message, stages again from that agent:
 * on its device when that's this composer's, the way the `@` menu
 * stages, and through this Mac from any other, the way a file dragged
 * from a timeline does. One staged on this device comes back as it is,
 * one from this Mac stages again, and any other becomes its marker's
 * words. A visual whose page came along stages that page again, and one
 * too large for a visual goes in as its fenced block.
 */
const placeFragment = (
	fragment: ClipboardFragment,
	{ attachments, gitPills, context, origin }: Placing
): Segment[] => {
	const { device } = context;
	const store = attachments.getState();
	const live = new Set(store.chips.map((chip) => chip.key));
	const files = new Map(fragment.files.map((file) => [file.key, file]));
	const keyFor = new Map<string, string | null>();
	for (const file of fragment.files) {
		const { found, page } = file;
		if (fragment.origin === origin && live.has(file.key)) keyFor.set(file.key, file.key);
		else if (page !== undefined)
			keyFor.set(file.key, fitsVisual(page) ? store.addPage(context, file.name, page) : null);
		else if (found && found.device === device)
			keyFor.set(
				file.key,
				store.addPaths(context, [found.path], { foundIn: found.agent })[0] ?? null
			);
		else if (found)
			keyFor.set(
				file.key,
				store.addFromAgent(context, {
					agent: found.agent,
					path: found.path,
					name: file.name,
					kind: file.kind,
					bytes: null,
					mime: null,
				})
			);
		else if (file.ref && file.device === device)
			keyFor.set(file.key, store.restore([file.ref])[0] ?? null);
		else if (file.source)
			keyFor.set(file.key, store.addPaths(context, [file.source])[0] ?? null);
		else keyFor.set(file.key, null);
	}
	const segments = fragment.segments.map((segment): Segment => {
		if (segment.type !== "pill") return segment;
		const { pill } = segment;
		if (pill.type === "visual") {
			const key = keyFor.get(pill.key);
			if (key) return { type: "pill", pill: { ...pill, key } };
			const page = files.get(pill.key)?.page;
			return {
				type: "text",
				text:
					page === undefined
						? visualMarker(pill.title)
						: `\n${toVisualFence({ html: page, title: pill.title })}\n`,
			};
		}
		if (pill.type !== "attachment") return segment;
		const key = keyFor.get(pill.key);
		if (key) return { type: "pill", pill: { type: "attachment", key } };
		const file = files.get(pill.key);
		return { type: "text", text: file ? fileMarker(file, undefined) : "" };
	});
	gitPills.getState().seed(keyedGitItems(markedText(fragment.segments), fragment.gitItems));
	return segments;
};

/**
 * What a link to a pull request or issue becomes: its `#123` when it's in
 * the agent's repo, which the `#` lookup finds it in, and `owner/repo#123`
 * when it's in another. Null when neither lookup finds it.
 */
const linkedItem = async (
	source: DataSource,
	agent: AgentId,
	link: GitLink
): Promise<{ readonly text: string; readonly key: string; readonly item: GitItemRef } | null> => {
	const own = await source.findGitItems(agent, String(link.number)).catch(() => null);
	const target: GitTarget =
		own?.repo != null && sameRepo(own.repo, link.repo)
			? { repo: null, number: link.number }
			: link;
	const item =
		target.repo === null
			? own?.items.find((each) => each.number === link.number)
			: await gitFinder(source, agent)(target);
	if (!item) return null;
	const named = target.repo === null ? target : { repo: item.repo, number: item.number };
	return {
		text: `${named.repo === null ? "" : named.repo}#${named.number}`,
		key: gitKey(named),
		item,
	};
};

/** The composer for one agent, or nothing for an agent MaxHQ doesn't hold. */
export const Composer = ({ agent, device, queued, timeline, dropTarget }: ComposerProps) => {
	useI18n();
	const record = useView((state) => state.view.tables.agent.get(agent));
	if (!record || record.control === "observed") return null;
	return (
		<ComposerBody
			key={agent}
			agent={record}
			device={device}
			queued={queued}
			timeline={timeline}
			dropTarget={dropTarget}
		/>
	);
};

const ComposerBody = ({
	agent,
	device,
	queued,
	timeline,
	dropTarget,
}: {
	agent: Agent;
	device: DeviceId;
	queued: readonly TimelineItem[];
	timeline: TimelineStore;
	dropTarget: Element | null | undefined;
}) => {
	const { language } = useI18n();
	const services = useServices();
	const { source } = services;
	const native = useComposerNative();
	const paneFiles = usePaneFiles();
	const view = useView((state) => state.view);
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const where = deviceName(view, device);
	const turn = turnOf(agent.activity);
	const link = linkForDevice(view, device);
	const remote = link !== undefined && link.route.type !== "local";
	const linkDown = remote && link.state.state !== "connected";
	const now = useNow(PILL_CLOCK_MS);

	const [attachments] = useState(
		() =>
			keptValue<AttachmentsStore | null>(draftFilesKey(agent.id), null) ?? createAttachments()
	);
	const chips = useStore(attachments, (state) => state.chips);
	const samples = useStore(attachments, (state) => state.samples);
	const [gitPills] = useState(() => createGitPills(gitFinder(source, agent.id)));
	const hover = useGitHover();
	/** Names this composer in a copied draft, so a paste here reuses its files. */
	const [origin] = useState(() => `composer-${Math.random().toString(36).slice(2)}`);
	/** Set when Cancel upload stops a send that waits for the files. */
	const cancelled = useRef(false);
	/** The excerpts a send readies, which no pill holds but which stay until it ends. */
	const excerpting = useRef(new Set<string>());
	const [excerptKeys, setExcerptKeys] = useState<readonly string[]>([]);
	const editor = useRef<PromptEditorHandle>(null);
	const box = useRef<HTMLDivElement>(null);
	const target: UploadTarget | null = remote ? { type: "agent", agent: agent.id } : null;
	const context: StageContext = { source, device, native, target };
	const [draft, setDraft] = useKept<EditorChange>(draftKey(agent.id), EMPTY_CHANGE);
	/** What the box opens with: the draft kept while the app runs, or the one this Mac saved. */
	const [initial] = useState((): readonly Segment[] => {
		if (draft !== EMPTY_CHANGE) return draft.segments;
		const saved = draftsOf(services).get(agent.id);
		return saved ? placeFragment(saved, { attachments, gitPills, context, origin }) : [];
	});
	const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
	const [pending, setPending] = useState<ReadonlySet<string>>(() => new Set());
	const [error, setError] = useState<string | null>(null);
	const [waitingForFiles, setWaitingForFiles] = useState(false);
	const [sending, setSending] = useState(false);
	const [over, setOver] = useState(false);
	// The dragged agents over it, past its own agent.
	const [agentOver, setAgentOver] = useState<readonly Agent[] | null>(null);
	// The dragged note over it.
	const [noteOver, setNoteOver] = useState<NoteRef | null>(null);
	const [quoteOver, setQuoteOver] = useState(false);

	const waiting = useMemo(() => waitingMessages(queued, hidden), [queued, hidden]);
	/** The agent's own project, whose notes it reads itself rather than as copies. */
	const ownProject = view.tables.workspace.get(agent.workspace)?.project ?? null;
	/** Why this agent's maxd can't take note pills, or null when it can. */
	const notesBlocked = notePillBlocked(view, device);
	const placed = useMemo(() => chipsOf(chips, fileKeys(draft.segments)), [chips, draft.segments]);
	/** What a send waits on: the placed files, and the excerpts it readies. */
	const outgoing = useMemo(
		() => [...placed, ...chipsOf(chips, excerptKeys)],
		[chips, excerptKeys, placed]
	);

	useEffect(() => () => gitPills.getState().dispose(), [gitPills]);

	/** The agent as a pill, on the device its workspace is on. */
	const agentRef = (other: Agent): AgentRef =>
		agentPillOf(services.view.getState().view, other, device);

	/** A visual as its pill, its page staging as a file. */
	const placeVisual = (visual: VisualTransfer): Segment[] =>
		placeFragment(visualFragment(visual), { attachments, gitPills, context, origin });

	const toCaret: Put = (segments) => editor.current?.insertSegments(segments);
	const toDrop: Put = (segments) => editor.current?.dropSegments(segments);

	/** Adds files dropped from the Finder as pills. */
	const dropPaths = (paths: readonly string[], put = toDrop) =>
		put(filePills(attachments.getState().addPaths(context, paths)));

	/** Adds a file dragged out of a timeline as a pill. */
	const dropTimelineFile = (file: DraggedFile, put = toDrop) =>
		put(filePills([attachments.getState().addFromAgent(context, file)]));

	useEffect(() => {
		const element = dropTarget === undefined ? box.current : dropTarget;
		if (!element) return;
		const files = native.registerDropTarget(element, {
			onDrop: (paths) => dropPaths(paths),
			onHover: (on) => {
				setOver(on);
				if (!on) setQuoteOver(false);
			},
			// A pane takes files anywhere. A drag out of a timeline, which may be
			// the pane's own, it takes only over the box, but a quote from another
			// agent anywhere.
			takesPageDrag: (drag, at) => {
				const quote = quoteOfDrag(drag);
				const visual = visualOfDrag(drag) !== null;
				const own = quote === null || quote.agent === agent.id;
				const takes =
					(visual || quote !== null || timelineFileOf(drag) !== null) &&
					(visual || !own || element === box.current || isOver(box.current, at));
				if (takes) setQuoteOver(quote !== null);
				return takes;
			},
			onPageDrop: (drag) => {
				const visual = visualOfDrag(drag);
				if (visual !== null) {
					editor.current?.dropSegments(placeVisual(visual));
					return;
				}
				const quote = quoteOfDrag(drag);
				if (quote !== null) {
					editor.current?.dropSegments([{ type: "pill", pill: quote }]);
					return;
				}
				const file = timelineFileOf(drag);
				if (file !== null) dropTimelineFile(file);
			},
		});
		const others = (dragged: readonly Agent[]) => dragged.filter((a) => a.id !== agent.id);
		const agents = agentDrops.register(element, {
			accepts: (dragged) => {
				const carried = draggedAgentsOf(dragged);
				return carried === null || others(carried).length > 0;
			},
			onHover: (dragged) => {
				const carried = dragged && draggedAgentsOf(dragged);
				setAgentOver(carried && others(carried));
				setNoteOver(dragged && draggedNoteOf(dragged));
			},
			onDrop: (dragged) => {
				const note = draggedNoteOf(dragged);
				if (note !== null) {
					// A maxd too old for note pills says so, and the note stays out.
					const blocked = notePillBlocked(services.view.getState().view, device);
					if (blocked !== null) setError(blocked);
					else editor.current?.dropSegments([{ type: "pill", pill: note }]);
					return;
				}
				editor.current?.dropSegments(
					others(draggedAgentsOf(dragged) ?? []).map((other) => ({
						type: "pill",
						pill: agentRef(other),
					}))
				);
			},
		});
		return () => {
			files();
			agents();
		};
		// The pill reads the agent's workspace when it lands, so it needs no
		// update, and these make the context the files stage with.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [agent.id, attachments, device, dropTarget, native, remote, source]);

	// Quotes sent here from a timeline's menu or the sidebar, now and later,
	// go in at the caret the way a dropped one does.
	useEffect(() => {
		const take = () => {
			const handle = editor.current;
			if (!handle) return;
			const quotes = quoteInbox.take(agent.id);
			if (quotes.length === 0) return;
			const pills = quotes.flatMap(({ quote, visual }, index): Segment[] => [
				...(index > 0 ? [{ type: "text", text: " " } as const] : []),
				...(quote ? [{ type: "pill", pill: quote } as const] : placeVisual(visual)),
			]);
			handle.dropSegments(
				pills,
				quotes.some(({ focus }) => focus)
			);
		};
		take();
		return quoteInbox.watch(agent.id, take);
		// A visual stages with the context as it is when it lands.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [agent.id]);

	const factsOf = (key: string): FileFacts | undefined => {
		const chip = attachments.getState().chips.find((each) => each.key === key);
		return chip && { name: chip.name, kind: chip.kind };
	};

	/** A visual's page, by its file's key, when this composer has it. */
	const pageOf = (key: string): string | undefined =>
		attachments.getState().chips.find((each) => each.key === key)?.page;

	/**
	 * The draft as a fragment that keeps its pills, as a copy or a saved
	 * draft holds one. A copy carries each visual's page, so a paste
	 * anywhere stages it again, and a saved draft carries it only until it
	 * stages.
	 */
	const fragmentOf = (segments: readonly Segment[], copy = false): ClipboardFragment => {
		const store = attachments.getState();
		const files = fileKeys(segments).map((key): ClipboardFile => {
			const chip = store.chips.find((each) => each.key === key);
			if (!chip)
				return {
					key,
					name: "file",
					kind: "missing",
					source: null,
					ref: null,
					device: null,
				};
			const file: ClipboardFile = {
				key,
				name: chip.name,
				kind: chip.kind,
				source: chip.source,
				ref: chip.ref,
				device: chip.ref ? device : null,
			};
			return chip.page !== undefined && (copy || chip.ref === null)
				? { ...file, page: chip.page }
				: file;
		});
		return {
			version: 1,
			origin,
			segments,
			files,
			gitItems: shownItems(gitPills.getState(), markedText(segments)),
		};
	};

	/**
	 * Saves the draft on this Mac for the next run. While a walk through
	 * earlier messages shows one, the draft it put aside is the one saved.
	 */
	const saveDraft = (segments: readonly Segment[]) => {
		const walk = keptValue<HistoryPosition | null>(walkKey(agent.id), null);
		const shown =
			walk !== null && markedText(segments) === walk.recalled ? walk.draft : segments;
		draftsOf(services).save(agent.id, fragmentOf(shown));
	};

	// A file that stages after the last key is saved with its staged copy.
	useEffect(() => {
		let staged = stagedIds(attachments.getState().chips).join();
		return attachments.subscribe(({ chips }) => {
			const now = stagedIds(chips).join();
			if (now === staged) return;
			staged = now;
			const segments = editor.current?.segments();
			if (segments) saveDraft(segments);
		});
		// The save reads the walk and the pills as they are then.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [agent.id, attachments, device]);

	const changed = (change: EditorChange) => {
		setDraft(change);
		saveDraft(change.segments);
		keepDraft(draftFilesKey(agent.id), attachments);
		const store = attachments.getState();
		// The draft a walk through earlier messages put aside keeps its files for Down.
		const aside = keptValue<HistoryPosition | null>(walkKey(agent.id), null)?.draft ?? [];
		for (const key of releasedFiles({
			keys: store.chips.flatMap((chip) =>
				excerpting.current.has(chip.key) ? [] : [chip.key]
			),
			placed: new Set([...fileKeys(change.segments), ...fileKeys(aside)]),
			held: change.held,
		}))
			store.remove(key);
		gitPills.getState().want(gitNumbers(change.text));
	};

	const hide = (id: string, gone: boolean) =>
		setHidden((current) => {
			const next = new Set(current);
			if (gone) next.add(id);
			else next.delete(id);
			return next;
		});

	const mark = (id: string, busy: boolean) =>
		setPending((current) => {
			const next = new Set(current);
			if (busy) next.add(id);
			else next.delete(id);
			return next;
		});

	/** Runs an action and shows why it failed, if it did. Returns whether it went through. */
	const run = async (
		what: ComposerAction,
		action: () => Promise<ActionResult>
	): Promise<boolean> => {
		const failure = failureOf(await attempt(action), what);
		setError(failure);
		return failure === null;
	};

	/** The files the text names now. */
	const placedNow = (segments: readonly Segment[]): Chip[] =>
		chipsOf(attachments.getState().chips, fileKeys(segments));

	/**
	 * Takes each Markdown file a send copies, such as an agent's excerpt or
	 * a note from another project, and readies it on this agent's device, the
	 * way a pasted file goes. Resolves once each has staged or failed.
	 */
	const takeCopies = async <K,>(
		jobs: readonly (readonly [K, () => Promise<{ name: string; text: string }>])[]
	): Promise<[K, Excerpt][]> => {
		const taken = await Promise.all(
			jobs.map(async ([id, take]): Promise<[K, Excerpt]> => {
				try {
					const copy = await take();
					if (cancelled.current) return [id, { error: null }];
					const key = attachments
						.getState()
						.addBytes(
							context,
							copy.name,
							"text/markdown",
							new TextEncoder().encode(copy.text)
						);
					excerpting.current.add(key);
					setExcerptKeys([...excerpting.current]);
					return [id, { key }];
				} catch (error) {
					return [id, { error: error instanceof Error ? error.message : String(error) }];
				}
			})
		);
		const keys = taken.flatMap(([, excerpt]) => ("key" in excerpt ? [excerpt.key] : []));
		await attachments.getState().settled(keys);
		return taken;
	};

	/** Each agent's excerpt, taken on its own device. */
	const takeExcerpts = (refs: readonly AgentRef[]) =>
		takeCopies(
			refs.map(
				(ref) => [ref.agent, () => source.excerptAgent(ref.agent, EXCERPT_BUDGET)] as const
			)
		);

	/** Each note from another project, read where the app reads notes. */
	const takeNotes = (refs: readonly NoteRef[]) =>
		takeCopies(
			refs.map(
				(ref) =>
					[
						ref.note,
						async () => {
							const reach = notesReach(services.view.getState().view, ref.project);
							if (reach.device === null) throw new Error(reach.why ?? "");
							const read = await source.readNote(
								reach.device,
								ref.project,
								ref.note,
								null
							);
							if (read.status === "error") throw new Error(errorText(read.error));
							if (read.data.deleted) throw new Error(translate("It was deleted."));
							return { name: noteCopyName(read.data.title), text: read.data.body };
						},
					] as const
			)
		);

	/** The copy each pill goes with, or why one isn't ready. */
	const readyCopies = <K,>(
		wanted: readonly { readonly id: K; readonly title: string }[],
		copies: ReadonlyMap<K, Excerpt>
	) => {
		const { chips } = attachments.getState();
		const ready = new Map<K, AttachmentId>();
		const unready: ExcerptFailure[] = [];
		for (const { id, title } of wanted) {
			const copy = copies.get(id);
			const chip = copy && "key" in copy ? chipsOf(chips, [copy.key])[0] : undefined;
			if (chip?.state === "staged" && chip.ref) ready.set(id, chip.ref.id);
			else
				unready.push({
					title,
					error: copy && "error" in copy ? copy.error : (chip?.error ?? null),
				});
		}
		return { ready, unready };
	};

	const send = async (mode: SendMode) => {
		if (sending || turn === "offline" || !editor.current) return;
		if (isBlank({ segments: editor.current.segments(), gitItems: [] })) return;
		// A maxd too old for note pills would drop them, so the message waits.
		const blocked = notePillBlocked(services.view.getState().view, device);
		if (blocked !== null && hasNote(editor.current.segments())) {
			setError(blocked);
			return;
		}
		setError(null);
		setSending(true);
		cancelled.current = false;
		const excerpts = new Map<AgentId, Excerpt>();
		const noteCopies = new Map<NoteId, Excerpt>();
		// Notes from another project go as copies; with no project, every note does.
		const farOf = (segments: readonly Segment[]) => farNotes(segments, ownProject ?? "");
		try {
			// An Auto you didn't pick, as the device's default, asks first.
			const agreed = await agreedSend(services, agent);
			if (!agreed.go) {
				setError(agreed.failure);
				return;
			}
			// Files placed while others got ready wait their turn too, and so
			// does an agent on another device, whose excerpt is taken now, and
			// a note from another project, which is copied now.
			let segments = editor.current.segments();
			for (;;) {
				const wanted = farAgents(segments, device).filter(
					(ref) => !excerpts.has(ref.agent)
				);
				const notes = farOf(segments).filter((ref) => !noteCopies.has(ref.note));
				if (!anyStaging(placedNow(segments)) && wanted.length === 0 && notes.length === 0)
					break;
				setWaitingForFiles(true);
				const [, taken, copied] = await Promise.all([
					attachments.getState().settled(fileKeys(segments)),
					takeExcerpts(wanted),
					takeNotes(notes),
				]);
				for (const [id, excerpt] of taken) excerpts.set(id, excerpt);
				for (const [id, copy] of copied) noteCopies.set(id, copy);
				setWaitingForFiles(false);
				if (cancelled.current || !editor.current) return;
				segments = editor.current.segments();
			}
			const files = placedNow(segments);
			const failed = files.filter((chip) => chip.state === "failed");
			if (failed.length > 0) {
				setError(attachFailure(failed));
				return;
			}
			const { ready, unready } = readyCopies(
				farAgents(segments, device).map((ref) => ({ id: ref.agent, title: ref.title })),
				excerpts
			);
			if (unready.length > 0) {
				setError(excerptFailure(unready));
				return;
			}
			const notesReady = readyCopies(
				farOf(segments).map((ref) => ({ id: ref.note, title: shownTitle(ref.title) })),
				noteCopies
			);
			if (notesReady.unready.length > 0) {
				setError(copyFailure(notesReady.unready));
				return;
			}
			if (isBlank({ segments, gitItems: [] })) return;
			const ids = new Map(files.map((chip) => [chip.key, chip.ref?.id ?? null]));
			const text = markedText(segments);
			// A pull request or issue picked a while ago goes out as it is now.
			const shown = shownItems(gitPills.getState(), text);
			const find = gitFinder(source, agent.id);
			const gitItems = await refreshGitItems(
				shown,
				(item) => find(gitTargetOf(text, item)),
				Date.now()
			);
			if (!editor.current) return;
			gitPills.getState().seed(
				keyedGitItems(
					text,
					gitItems.filter((item, index) => item !== shown[index])
				)
			);
			// The space after a last pill doesn't go out.
			const input = withExcerpts(
				toUserInput(
					{ segments: trimEnd(segments), gitItems },
					(key): AttachmentId | null => ids.get(key) ?? null,
					// An older maxd gets a visual as its page's file.
					{ visuals: sendsVisuals(services.view.getState().view, device) }
				),
				ready,
				notesReady.ready
			);
			const sent = await run(mode === "steer" ? "steer" : "send", () =>
				source.sendInput(agent.id, input, mode)
			);
			if (!sent) return;
			if (agreed.failure) setError(agreed.failure);
			// A message brought back with Up goes out like any other, and the walk ends.
			keepDraft(walkKey(agent.id), null);
			if (!editor.current) {
				// The pane closed while it sent, so the draft it kept went out.
				if (keptValue(draftKey(agent.id), EMPTY_CHANGE).text === text) {
					setDraft(EMPTY_CHANGE);
					draftsOf(services).drop([agent.id]);
					for (const chip of files) attachments.getState().remove(chip.key);
				}
				return;
			}
			if (editor.current.text() === text) {
				editor.current.reset();
				return;
			}
			// Typed on while it sent, so keep the new words and drop what went.
			const keys = new Set(files.map((chip) => chip.key));
			editor.current.removeFiles(keys);
			for (const key of keys) attachments.getState().remove(key);
		} finally {
			// An excerpt is only for this send, whether it went or not.
			const store = attachments.getState();
			for (const key of excerpting.current) store.remove(key);
			excerpting.current.clear();
			setExcerptKeys([]);
			setWaitingForFiles(false);
			setSending(false);
		}
	};

	const stop = () => void run("stop", () => source.interrupt(agent.id));
	useStopKey(dropTarget ?? null, canStop(turn) ? stop : null);

	/** Stops every upload, and the send that waits for them, so the words stay. */
	const cancelUploads = () => {
		cancelled.current = true;
		const store = attachments.getState();
		const uploading = store.chips.filter((chip) => chip.state === "uploading");
		store.cancelUploads();
		editor.current?.removeFiles(new Set(uploading.map((chip) => chip.key)));
		editor.current?.focus();
	};

	const retryUploads = () => {
		const store = attachments.getState();
		for (const chip of placed)
			if (chip.state === "failed" && chip.retryable) store.retry(chip.key);
	};

	const remove = async (message: QueuedMessage) => {
		mark(message.id, true);
		if (await run("unqueue", () => source.cancelQueued(agent.id, message.id)))
			hide(message.id, true);
		mark(message.id, false);
	};

	/** Puts a message the queue gave back in the box, before anything already there. */
	const restore = (message: QueuedMessage) => {
		const excerpts = excerptIds(message.refs);
		const files = message.attachments.filter((ref) => !excerpts.has(ref.id));
		const keys = attachments.getState().restore(files);
		const keyOf = new Map(files.map((ref, index) => [ref.id, keys[index]]));
		const restored = fromUserInput(inputOf(message), (id) => keyOf.get(id) ?? id);
		gitPills.getState().seed(keyedGitItems(markedText(restored.segments), restored.gitItems));
		const current = editor.current?.segments() ?? [];
		editor.current?.setSegments(
			isBlank({ segments: current, gitItems: [] })
				? restored.segments
				: [...restored.segments, { type: "text", text: "\n\n" }, ...current]
		);
	};

	const edit = async (message: QueuedMessage) => {
		hide(message.id, true);
		const taken = await run("unqueue", () => source.cancelQueued(agent.id, message.id));
		if (!taken) {
			hide(message.id, false);
			return;
		}
		restore(message);
	};

	/** Steers a queued message into the turn, or sends it now past the prompts that wait. */
	const steerNow = async (message: QueuedMessage) => {
		const mode = nowMode(turn);
		if (mode === null) return;
		const action = mode === "steer" ? "steer" : "send";
		mark(message.id, true);
		const taken = await run(action, () => source.cancelQueued(agent.id, message.id));
		if (taken) {
			hide(message.id, true);
			const steered = await run(action, () =>
				source.sendInput(agent.id, inputOf(message), mode)
			);
			// Keep the words, since the queue no longer has them.
			if (!steered) restore(message);
		}
		mark(message.id, false);
	};

	const recall = (): boolean => {
		const last = waiting.at(-1);
		if (!last) return false;
		void edit(last);
		return true;
	};

	/**
	 * Up and Down. Up in an empty box takes back the last queued message.
	 * With nothing queued, they walk through the messages you sent, as
	 * history.ts tells, and each comes back with its pills.
	 */
	const historyKey = (direction: HistoryDirection, caret: HistoryCaret): boolean => {
		const box = editor.current;
		if (!box) return false;
		const text = box.text();
		if (direction === "backward" && text === "" && waiting.length > 0) return recall();
		const key = walkKey(agent.id);
		const step = stepHistory({
			direction,
			entries: () => historyEntries(timeline.getState()),
			position: keptValue<HistoryPosition | null>(key, null),
			text,
			segments: box.segments(),
			caret,
			queued: waiting.length > 0,
		});
		if (!step) return false;
		if (step.type === "draft") {
			keepDraft(key, null);
			box.setSegments(step.draft);
		} else {
			const { entry, draft } = step;
			const position = { entryId: entry.id, prompt: entry.prompt, recalled: "", draft };
			// Kept before the message goes in, so the draft's files stay.
			keepDraft(key, position);
			const place = { agent: agent.id, device };
			box.setSegments(fragmentSegments(messageFragment(entry.message, place)));
			keepDraft(key, { ...position, recalled: box.text() });
			saveDraft(box.segments());
		}
		// A word the step left at the caret opens no menu.
		pillMenu.shutAt(box.text());
		return true;
	};

	const pick = async () => {
		try {
			const paths = await native.pickAttachments();
			const keys = attachments.getState().addPaths(context, paths);
			editor.current?.insertSegments(filePills(keys));
		} catch (failure) {
			setError(intakeFailure(failure, "pick"));
		}
		editor.current?.focus();
	};

	const names = () => attachments.getState().chips.map((chip) => chip.name);

	/**
	 * Adds pasted or dropped files as pills. Every file reads first, so each
	 * pill goes in as soon as its file is added.
	 */
	const addFiles = async (files: readonly File[], put = toCaret) => {
		const read = await Promise.all(
			files.map(async (file) => ({ file, bytes: new Uint8Array(await file.arrayBuffer()) }))
		);
		const store = attachments.getState();
		const keys = read.map(({ file, bytes }) =>
			store.addBytes(
				context,
				file.name || "pasted-file",
				file.type || "application/octet-stream",
				bytes
			)
		);
		put(filePills(keys));
	};

	const pasteFiles = async (files: readonly File[], text: string, put = toCaret) => {
		try {
			const paths = await native.readClipboardFiles();
			if (paths.length > 0) {
				put(filePills(attachments.getState().addPaths(context, paths)));
				return;
			}
			const png = await native.readClipboardImage();
			if (png) {
				const key = attachments
					.getState()
					.addBytes(context, nextPastedImageName(names()), "image/png", png);
				put(filePills([key]));
				return;
			}
		} catch (failure) {
			setError(intakeFailure(failure, "paste"));
		}
		if (files.length > 0) await addFiles(files, put);
		else if (!pasteLink(text) && text !== "") editor.current?.insert(text);
	};

	/**
	 * Types a paste that's a link to a pull request or issue and nothing
	 * else, and puts the item's `#123` in its place once the lookup finds
	 * it, if the link is still there as pasted. Returns whether it took the
	 * paste.
	 */
	const pasteLink = (text: string): boolean => {
		const link = gitLinkOf(text);
		const box = editor.current;
		if (!link || !box) return false;
		const id = box.insertLink(text.trim());
		if (id === null) return true;
		linkedItem(source, agent.id, link).then(
			(found) => {
				const swapped = editor.current?.swapLink(id, found?.text ?? null) ?? false;
				if (found && swapped) gitPills.getState().seed(new Map([[found.key, found.item]]));
			},
			() => editor.current?.swapLink(id, null)
		);
		return true;
	};

	const fragmentSegments = (fragment: ClipboardFragment): Segment[] =>
		placeFragment(fragment, { attachments, gitPills, context, origin });

	const paste = (data: DataTransfer): boolean => {
		const fragment = readFragment(data);
		if (fragment) {
			editor.current?.insertSegments(fragmentSegments(fragment));
			return true;
		}
		// A visual's own type, or text that is one visual's block and nothing else.
		const visual = readVisual(data);
		if (visual) {
			editor.current?.insertSegments(placeVisual(visual));
			return true;
		}
		const text = data.getData("text/plain");
		const files = Array.from(data.files);
		const types = Array.from(data.types);
		if (files.length > 0 || types.includes("Files") || types.includes("text/uri-list")) {
			void pasteFiles(files, text);
			return true;
		}
		if (text !== "" && pastesAsFile(text)) {
			const name = nextPastedTextFileName(names());
			const key = attachments
				.getState()
				.addBytes(context, name, "text/plain", new TextEncoder().encode(text));
			editor.current?.insertSegments(filePills([key]));
			return true;
		}
		return pasteLink(text);
	};

	// Files dropped or pasted on an answer field in the pane go in as they
	// would here, where a drop goes, while the focus stays in that field.
	const handIn = (file: PaneFile) => {
		const put: Put = (segments) => editor.current?.dropSegments(segments, false);
		switch (file.type) {
			case "paths":
				return dropPaths(file.paths, put);
			case "timeline":
				return dropTimelineFile(file.file, put);
			case "dropped":
				return void addFiles(file.files, put);
			case "pasted":
				return void pasteFiles(file.files, "", put);
		}
	};
	const handedIn = useRef(handIn);
	useLayoutEffect(() => {
		handedIn.current = handIn;
	});
	useEffect(() => paneFiles?.take((file) => handedIn.current(file)), [paneFiles]);

	/**
	 * A copy's plain form for other apps, and the fragment that keeps its
	 * pills here. A copy of one visual and nothing else goes under the
	 * visual's own type too, so a note takes it.
	 */
	const copy = (segments: Segment[]) => {
		const kept = segments.filter((each) => each.type === "pill" || each.text.trim() !== "");
		const only = kept.length === 1 ? kept[0] : undefined;
		const visual = only?.type === "pill" && only.pill.type === "visual" ? only.pill : null;
		const html = visual ? pageOf(visual.key) : undefined;
		return {
			plain: plainText(segments, factsOf, pageOf),
			fragment: encodeFragment(fragmentOf(segments, true)),
			...(visual && html !== undefined
				? {
						extra: {
							[VISUAL_MIME]: writeVisualTransfer({
								html,
								title: visual.title,
								from: visual.from,
							}),
						},
					}
				: {}),
		};
	};

	// The menu for the word at the caret, unless Escape closed it.
	const pillMenu = usePillMenu({
		editor,
		text: draft.text,
		// A draft this Mac saved opens with its menu shut, as a kept one does.
		shut: draft === EMPTY_CHANGE ? markedText(initial) : draft.text,
		caret: draft.caret,
		agent: agent.id,
		kinds: ALL_TRIGGERS,
		// The path is relative to the folder the search looked in, which only
		// the agent's device knows, so it stages from there.
		onPath: (path) =>
			filePills(attachments.getState().addPaths(context, [path], { foundIn: agent.id })),
		agentsOn: null,
		// Notes join the `@` menu once this agent's maxd can take them.
		notes: notesBlocked === null ? { own: ownProject, only: false } : null,
		gitPills,
		agentPill: agentRef,
	});

	const pills = useMemo(
		(): PillsContextValue => ({
			attachments,
			native,
			numbers: fileNumbers(draft.segments, (key) => {
				const chip = chips.find((each) => each.key === key);
				return chip && { name: chip.name, kind: chip.kind };
			}),
			now,
			agentOf: (id) => view.tables.agent.get(id) ?? null,
			deviceName: (id) => deviceName(view, id),
			agent: agent.id,
		}),
		[agent.id, attachments, chips, draft.segments, native, now, view]
	);

	const hasContent = !isBlank({ segments: draft.segments, gitItems: [] });
	// The overlay hides the text, so it says where a drop goes once there's text.
	const atCaret = hasContent ? translate(" at the cursor") : "";
	const ready = hasContent && turn !== "offline" && !sending;
	const sends: Sends =
		turn === "busy"
			? "steer"
			: turn === "prompt"
				? "now"
				: turn === "waiting"
					? "queue"
					: "send";
	const models = modelsOf(view, account);
	const effortShows = canSetEffort(view, agent, device, models);
	const attached: Attached | null =
		placed.length > 0
			? { length: placed.length, count: chipCount(placed), summary: chipSummary(placed) }
			: null;
	const waitingNote = waitingForFiles
		? anyUploading(outgoing)
			? translate("Sends when the upload finishes")
			: translate("Sends when the files are ready")
		: null;
	// What the footer shows, so it measures itself again when that changes.
	const footerContent = [
		language,
		attached?.summary,
		agent.model,
		agent.effort,
		agent.permission && runningMode(agent.permission),
		sends,
		canStop(turn),
		ready,
		waitingNote,
	].join("|");

	return (
		<PillsContext value={pills}>
			<div className="composer">
				<QueuedList
					messages={waiting}
					now={nowMode(turn)}
					pending={pending}
					onSteer={(message) => void steerNow(message)}
					onEdit={(message) => void edit(message)}
					onRemove={(message) => void remove(message)}
				/>
				<div
					ref={box}
					className="relative rounded-[10px] border border-input bg-subpanel focus-within:border-primary focus-within:shadow-focus"
					onBlur={(event) => {
						// Clicking away closes the menu, though a click in it keeps the focus.
						if (!event.currentTarget.contains(event.relatedTarget)) pillMenu.close();
					}}
				>
					{pillMenu.element}
					{remote && (
						<UploadStrip
							chips={outgoing}
							samples={samples}
							device={where}
							linkDown={linkDown}
							onCancel={cancelUploads}
							onRetry={retryUploads}
						/>
					)}
					{turn === "prompt" && (
						<div className="flex h-[30px] items-center gap-2 border-b border-secondary px-3 text-[11.5px] text-muted-foreground">
							<Clock aria-hidden className="size-3 shrink-0" strokeWidth={2.2} />
							<button
								type="button"
								className="shrink-0 cursor-pointer rounded-[4px] font-medium text-status-ready-text outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
								onClick={() =>
									askForRequest(
										agent.id,
										agent.activity.state === "needs_input"
											? agent.activity.attention
											: null
									)
								}
							>
								{translate("Go to the request")}
							</button>
							<span className="truncate">
								{translate(
									"Anything you send now waits until {{value1}}'s next turn.",
									{ value1: VENDOR_NAME[agent.vendor] }
								)}
							</span>
						</div>
					)}
					<PromptEditor
						ref={editor}
						native={native}
						gitPills={gitPills}
						label={translate("Message {{title}}", { title: agent.title })}
						placeholder={placeholderFor(agent.activity, agent.title, where)}
						menuAttributes={pillMenu.attributes}
						initial={initial}
						onEnter={(withModifier) =>
							void send((withModifier ? nowMode(turn) : null) ?? "queue")
						}
						onHistoryKey={historyKey}
						onAttach={() => void pick()}
						onPaste={paste}
						onDropFiles={(files) => void addFiles(files, toDrop)}
						onDropTimelineFile={(file) => dropTimelineFile(file)}
						onChange={changed}
						onCopy={copy}
						onMenuKey={pillMenu.onKey}
						onGitHover={hover.hover}
						onDropQuote={(quote) =>
							editor.current?.dropSegments([{ type: "pill", pill: quote }])
						}
						onDropVisual={(visual) => editor.current?.dropSegments(placeVisual(visual))}
					/>
					<GitPillPreview
						store={gitPills}
						shown={hover.shown}
						now={now}
						onHold={hover.hold}
						onClose={hover.close}
					/>
					{error !== null && (
						<p role="alert" className="px-3 pb-1 text-[11.5px] text-status-error-text">
							{error}
						</p>
					)}
					<ImageNote
						account={account}
						model={agent.model}
						choices={models}
						chips={placed}
					/>
					<FooterRow
						shows={{
							sized: attached !== null && attached.summary !== attached.count,
							attachments: attached !== null,
							effort: effortShows,
							permission: agent.permission !== null && agent.permission !== undefined,
						}}
						content={footerContent}
					>
						{(folded) => (
							<>
								<AttachButton
									attached={attached}
									folded={folded}
									onPick={() => void pick()}
								/>
								<ModelPicker
									vendor={agent.vendor}
									model={agent.model}
									account={account}
									choices={models}
									moves={movesOf(view, agent)}
									folded={folded.has("model")}
									onPick={(model, to) =>
										void run("model", () =>
											source.setModel(agent.id, model, to)
										)
									}
								/>
								{effortShows && (
									<EffortPicker
										effort={agent.effort ?? null}
										model={agent.model}
										choices={models}
										folded={folded.has("effort")}
										onPick={(effort) =>
											void run("effort", () =>
												source.setEffort(agent.id, effort)
											)
										}
									/>
								)}
								{agent.permission && (
									<PermissionPicker
										vendor={agent.vendor}
										acp={account?.acp}
										permission={agent.permission}
										folded={folded.has("permission")}
										onPick={(mode) =>
											pickMode(
												services,
												mode,
												() =>
													void run("permission", () =>
														source.setPermission(agent.id, mode)
													)
											)
										}
									/>
								)}
								<SendButtons
									sends={sends}
									ready={ready}
									waiting={waitingNote}
									stops={canStop(turn)}
									folded={folded}
									onSend={(mode) => void send(mode)}
									onStop={stop}
								/>
							</>
						)}
					</FooterRow>
					{(over || agentOver || noteOver) && (
						<div
							aria-hidden
							className="pointer-events-none absolute -inset-px grid place-items-center rounded-[10px] border-[1.5px] border-dashed border-primary bg-subpanel/90"
						>
							<div className="flex flex-col items-center gap-1 text-center">
								{agentOver?.[0] ? (
									<VendorIcon
										vendor={agentOver[0].vendor}
										size={20}
										className="text-primary"
									/>
								) : noteOver ? (
									<StickyNote className="size-5 text-primary" />
								) : quoteOver ? (
									<TextQuote className="size-5 text-primary" />
								) : (
									<Upload className="size-5 text-primary" />
								)}
								<span className="text-[13.5px] font-semibold text-foreground-strong">
									{agentOver
										? translate("Drop to add {{draggedName}}{{atCaret}}", {
												draggedName: draggedName(agentOver),
												atCaret: atCaret,
											})
										: noteOver
											? (notesBlocked ??
												translate("Drop to add {{shownTitle}}{{atCaret}}", {
													shownTitle: shownTitle(noteOver.title),
													atCaret: atCaret,
												}))
											: quoteOver
												? translate("Drop to add the quote{{atCaret}}", {
														atCaret: atCaret,
													})
												: translate("Drop to attach{{atCaret}}", {
														atCaret: atCaret,
													})}
								</span>
								<span className="text-[11.5px] text-muted-foreground">
									{translate("For ")}
									{agent.title} {translate(" on ")}
									{where}
								</span>
							</div>
						</div>
					)}
				</div>
			</div>
		</PillsContext>
	);
};
