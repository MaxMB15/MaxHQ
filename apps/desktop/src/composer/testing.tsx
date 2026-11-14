// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Fakes for the composer's tests: a data source that records what the
// composer asks of it, native calls a test answers by hand, and the
// browser features jsdom lacks.

import type { Editor } from "@tiptap/core";
import { act, render, screen } from "@testing-library/react";
import { vi } from "vitest";

import { FILE_DRAG_TYPE } from "@/agent-view/file-drag";
import { ServicesProvider } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import type {
	Account,
	Agent,
	AttachmentRef,
	Delta,
	DeviceLink,
	Entity,
	InlineRef,
	TimelineItem,
} from "@maxhq/shared/bindings";
import { agent as buildAgent, device, entities, link } from "@maxhq/shared/test/build";
import { remoteCalls } from "@maxhq/shared/test/remote";
import { applyUpdate, EMPTY_TIMELINE } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { Composer } from "./composer";
import {
	type ComposerNative,
	ComposerNativeProvider,
	type DropHandlers,
	guessPath,
	type Misspelling,
} from "./native";

export const AGENT = "a1";
export const DEVICE = "dev-mac";

/** A remote device's ID, reached over SSH as devbox. */
export const REMOTE = "dev-devbox";

/** The link to devbox, connected unless a test says otherwise. */
export const remoteLink = (over: Partial<DeviceLink> = {}): DeviceLink =>
	link({ id: "devbox", device: REMOTE, ...over });

export const DONE: ActionResult = { status: "ok", data: { status: "done" } };

/** A picture dragged out of agent a2's timeline, as the drop router hands it over. */
export const SHOT_DRAG = new Map([
	[
		FILE_DRAG_TYPE,
		JSON.stringify({
			device: REMOTE,
			agent: "a2",
			path: "/attachments/a2/shot.png",
			name: "shot.png",
			kind: "image",
			mime: "image/png",
			bytes: 48_213,
		}),
	],
]);

/** A clipboard as a paste event carries it. */
export const clipboard = ({ text = "", files = [] as File[], types = [] as string[] }) => ({
	getData: (format: string) => (format === "text/plain" ? text : ""),
	files,
	types: text === "" ? types : ["text/plain", ...types],
});

/** A promise the test settles by hand. */
export const deferred = <T,>() => {
	let resolve!: (value: T) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<T>((yes, no) => {
		resolve = yes;
		reject = no;
	});
	return { promise, resolve, reject };
};

/** A staged attachment named after its file. */
export const attachment = (name: string, over: Partial<AttachmentRef> = {}): AttachmentRef => ({
	id: `att-${name}`,
	name,
	kind: "file",
	bytes: 2048,
	mime: null,
	path: `/staged/${name}`,
	thumbnail: null,
	...over,
});

/** What staging a path or pasted bytes comes back with. */
export const staged = (ref: AttachmentRef): ActionResult => ({
	status: "ok",
	data: { status: "staged", attachment: ref },
});

const nameOf = (path: string) => guessPath(path).name;

/** A data source whose actions are spies, and whose agent a test can change. */
export const fakeSource = (initial: {
	agent: Agent;
	account?: Account;
	links?: DeviceLink[];
	/** More the view knows from the start, such as other agents and their workspaces. */
	more?: Entity[];
}) => {
	let emit: (delta: Delta) => void = () => {};
	let seq = 1;
	const source = {
		kind: "mock",
		snapshot: async () => ({
			seq: 1,
			entities: [
				...entities({
					agents: [initial.agent],
					accounts: initial.account ? [initial.account] : [],
					devices: [
						device({ id: DEVICE, name: "Mac" }),
						device({ id: REMOTE, name: "devbox" }),
					],
					links: initial.links ?? [],
				}),
				...(initial.more ?? []),
			],
		}),
		subscribe: async (onDelta) => {
			emit = onDelta;
			return () => {};
		},
		onFocusAgent: async () => () => {},
		appInfo: async () => null,
		markSeen: async () => ({ status: "ok", data: null }),
		refreshUsage: async () => ({ status: "ok", data: null }),
		refreshLinks: async () => ({ status: "ok", data: null }),
		setShown: async () => {},
		openTimeline: async () => () => {},
		loadOlder: async () => ({ status: "ok", data: null }),
		openWorkflow: async () => () => {},
		nodeDetail: async () => ({ status: "ok", data: { lists: [] } }),
		startAgent: async () => DONE,
		sendInput: vi.fn<DataSource["sendInput"]>(async () => DONE),
		cancelQueued: vi.fn<DataSource["cancelQueued"]>(async () => DONE),
		interrupt: vi.fn<DataSource["interrupt"]>(async () => DONE),
		setPermission: vi.fn<DataSource["setPermission"]>(async () => DONE),
		setModel: vi.fn<DataSource["setModel"]>(async () => DONE),
		setEffort: vi.fn<DataSource["setEffort"]>(async () => DONE),
		stopAgent: async () => DONE,
		setInbox: async (agents) => agents.map(() => DONE),
		continueHere: async () => DONE,
		takeOver: async () => DONE,
		answer: async () => DONE,
		stageAttachments: vi.fn<DataSource["stageAttachments"]>(async (_device, paths) =>
			paths.map((path) => staged(attachment(nameOf(path))))
		),
		stagePasted: vi.fn<DataSource["stagePasted"]>(async (_device, name, mime, bytes) =>
			staged(
				attachment(name, {
					mime,
					bytes: bytes.byteLength,
					kind: mime.startsWith("image/") ? "image" : "file",
				})
			)
		),
		setDeviceSettings: vi.fn<DataSource["setDeviceSettings"]>(async () => DONE),
		...remoteCalls(),
		upload: vi.fn<DataSource["upload"]>(async (_device, _target, paths) =>
			paths.map((path) => staged(attachment(nameOf(path))))
		),
		uploadPasted: vi.fn<DataSource["uploadPasted"]>(
			async (_device, _target, name, mime, bytes) =>
				staged(attachment(name, { mime, bytes: bytes.byteLength }))
		),
		attachFromAgent: vi.fn<DataSource["attachFromAgent"]>(
			async (_device, _target, _from, path) => staged(attachment(nameOf(path)))
		),
		cancelTransfer: vi.fn<DataSource["cancelTransfer"]>(async () => {}),
		findPaths: vi.fn<DataSource["findPaths"]>(async () => []),
		stageFound: vi.fn<DataSource["stageFound"]>(async (_agent, paths) =>
			paths.map((path) => staged(attachment(nameOf(path))))
		),
		findGitItems: vi.fn<DataSource["findGitItems"]>(async () => ({
			repo: "owner/repo",
			items: [],
			unavailable: null,
		})),
		excerptAgent: vi.fn<DataSource["excerptAgent"]>(async (agent) => ({
			name: `agent-${agent}.md`,
			text: `# Agent ${agent}\n`,
			messages: 1,
			omitted: 0,
		})),
		findInTimeline: vi.fn<DataSource["findInTimeline"]>(async () => ({
			total: 0,
			active: 0,
			found: null,
			near: [],
		})),
	} satisfies DataSource;

	/** Upserts entities, the way a delta from maxd would. */
	const upsert = (values: Entity[]) =>
		act(() => {
			seq += 1;
			emit({ seq, changes: values.map((value) => ({ op: "upsert", value })) });
		});

	/** Replaces the agent, the way a delta from maxd would. */
	const update = (agent: Agent) => upsert(entities({ agents: [agent] }));

	return { source, update, upsert };
};

/** Native calls that find nothing until a test says otherwise, and keep the drop target's handlers. */
export const fakeNative = ({ spelling = false }: { spelling?: boolean } = {}) => {
	let drop: DropHandlers | null = null;
	const native = {
		hasNativeSpelling: () => spelling,
		checkSpelling: vi.fn<ComposerNative["checkSpelling"]>(async () => []),
		spellingMenu: vi.fn<ComposerNative["spellingMenu"]>(async () => ({ kind: "none" })),
		pickAttachments: vi.fn<ComposerNative["pickAttachments"]>(async () => []),
		readClipboardFiles: vi.fn<ComposerNative["readClipboardFiles"]>(async () => []),
		readClipboardImage: vi.fn<ComposerNative["readClipboardImage"]>(async () => null),
		describePaths: vi.fn<ComposerNative["describePaths"]>(async (paths) =>
			paths.map(guessPath)
		),
		thumbnail: vi.fn<ComposerNative["thumbnail"]>(async () => null),
		registerDropTarget: vi.fn<ComposerNative["registerDropTarget"]>((_element, handlers) => {
			drop = handlers;
			return () => {
				drop = null;
			};
		}),
	} satisfies ComposerNative;
	return {
		native,
		/** The handlers the composer gave its drop target, which always takes files. */
		drop: (): DropHandlers & Required<Pick<DropHandlers, "onDrop">> => {
			const { onDrop } = drop ?? {};
			if (!drop || !onDrop) throw new Error("No drop target is registered");
			return { ...drop, onDrop };
		},
	};
};

/** A checker that flags each word in `wrong`, with offsets the way the native one gives them. */
export const checkerFor =
	(wrong: readonly string[]) =>
	async (text: string): Promise<Misspelling[]> =>
		[...text.matchAll(/[\p{L}']+/gu)]
			.filter((match) => wrong.includes(match[0]))
			.map((match) => ({
				start: match.index,
				end: match.index + match[0].length,
				word: match[0],
				language: "en",
			}));

/** A held agent called "Fix login", idle, asking before every change. */
export const heldAgent = (over: Partial<Agent> = {}): Agent =>
	buildAgent({
		id: AGENT,
		control: "held",
		title: "Fix login",
		permission: { requested: "ask", effective: null },
		...over,
	});

/** A message waiting for the turn to end. */
export const queuedMessage = (
	id: string,
	text: string,
	ordinal: number,
	attachments: AttachmentRef[] = []
): TimelineItem => ({
	id,
	agent: AGENT,
	turn: null,
	parent: null,
	ordinal,
	status: "pending",
	started_at: null,
	completed_at: null,
	kind: "user_message",
	text,
	attachments,
	delivery: "queued",
});

/** A message the agent took, with the files and pills it went with. */
export const sentMessage = (
	id: string,
	text: string,
	ordinal: number,
	attachments: AttachmentRef[] = [],
	refs?: InlineRef[]
): TimelineItem =>
	timelineItem(
		{ kind: "user_message", text, attachments, refs, delivery: "sent" },
		{ id, agent: AGENT, ordinal }
	);

/** Renders the composer for an agent, with the fakes behind it. */
export const renderComposer = ({
	agent = heldAgent(),
	account,
	queued = [],
	sent = [],
	native = fakeNative(),
	dropTarget,
	links,
	more,
	on = DEVICE,
}: {
	agent?: Agent;
	account?: Account;
	/** More the view knows from the start, such as the agent's workspace. */
	more?: Entity[];
	queued?: TimelineItem[];
	/** The agent's timeline, such as the messages Up brings back. */
	sent?: TimelineItem[];
	native?: ReturnType<typeof fakeNative>;
	dropTarget?: Element | null;
	/** The device links the view knows, such as `remoteLink()` for an agent on devbox. */
	links?: DeviceLink[];
	/** The device the agent runs on. */
	on?: string;
} = {}) => {
	const fake = fakeSource({ agent, account, links, more });
	const timeline = createTimelineStore();
	timeline.setState(
		applyUpdate(EMPTY_TIMELINE, { type: "reset", seq: 1, items: sent, has_older: false }),
		true
	);
	const tree = (items: TimelineItem[]) => (
		<ServicesProvider source={fake.source}>
			<ComposerNativeProvider value={native.native}>
				<Composer
					agent={agent.id}
					device={on}
					queued={items}
					timeline={timeline}
					dropTarget={dropTarget}
				/>
			</ComposerNativeProvider>
			<ConfirmHost />
		</ServicesProvider>
	);
	const view = render(tree(queued));
	return {
		...fake,
		native: native.native,
		drop: native.drop,
		timeline,
		rerender: (items: TimelineItem[]) => view.rerender(tree(items)),
	};
};

/** The TipTap editor behind a text box. */
export const editorOf = (textbox: HTMLElement): Editor => {
	const editor = (textbox as HTMLElement & { editor?: Editor }).editor;
	if (!editor) throw new Error("The text box has no editor");
	return editor;
};

/** The composer's text box, once the agent has loaded. */
export const findTextbox = () => screen.findByRole("textbox", { name: /^Message / });

/** Puts text in the composer as if typed. */
export const type = (textbox: HTMLElement, text: string) =>
	act(() => {
		editorOf(textbox).commands.insertContent(text);
	});

/** What each pill in the text box says to a screen reader, in order. */
export const pillsIn = (textbox: HTMLElement) =>
	[...textbox.querySelectorAll(".pill")].map((pill) => pill.getAttribute("aria-label") ?? "");

/** The text in the composer, one line per paragraph. */
export const textIn = (textbox: HTMLElement) => editorOf(textbox).getText({ blockSeparator: "\n" });

type Rect = { left: number; top: number; right: number; bottom: number };

/** Where each word sits on screen, for the fake `Range.getClientRects`. */
export const wordRects = new Map<string, Rect>();

const rectList = (rects: Rect[]): DOMRectList =>
	Object.assign(
		rects.map((rect) => ({ ...rect, x: rect.left, y: rect.top, width: 0, height: 0 })),
		{ item: (index: number) => rects[index] ?? null }
	) as unknown as DOMRectList;

class FakeHighlight extends Set<AbstractRange> {
	priority = 0;
	type = "highlight";
}

/** Adds the client rects, highlights and object URLs jsdom lacks. */
export const installBrowserFakes = () => {
	Range.prototype.getClientRects = function (this: Range) {
		const rect = wordRects.get(this.toString());
		return rectList(rect ? [rect] : []);
	};
	Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
	Element.prototype.scrollIntoView = () => {};
	const scope = globalThis as unknown as {
		CSS?: { highlights?: Map<string, unknown> };
		Highlight?: unknown;
	};
	if (!scope.CSS)
		Object.defineProperty(globalThis, "CSS", { value: {}, configurable: true, writable: true });
	Object.defineProperty(scope.CSS, "highlights", { value: new Map(), configurable: true });
	scope.Highlight = FakeHighlight;
	let next = 0;
	URL.createObjectURL = () => `blob:fake/${++next}`;
	URL.revokeObjectURL = () => {};
};

/** The ranges every composer underlines now. */
export const underlined = (): string[] =>
	[...((CSS.highlights.get("misspelled") as unknown as Set<Range> | undefined) ?? [])].map(
		(range) => range.toString()
	);
