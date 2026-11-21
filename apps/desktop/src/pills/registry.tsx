// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the registry, and
// the details of a pull request's pill, taken to issues too.
// Each kind says how its pill looks and what its preview shows, so the
// composer and the sent message draw a piece of context the same way.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import type { Agent, AttachmentKind, NoteSummary, Timestamp } from "@maxhq/shared/bindings";
import type {
	AgentRef,
	GitItemRef,
	NoteRef,
	OutputRef,
	QuoteRef,
	SessionRef,
} from "@maxhq/shared/pills/draft";
import {
	type FileProgress,
	fileAria,
	fileLabel,
	gitItemLabel,
	gitKindWords,
	gitStateWords,
	middleTruncate,
	noteLabel,
	oneLine,
	quoteLabel,
} from "@maxhq/shared/pills/label";
import { ACTIVITY_LABEL, agentLook } from "@maxhq/shared/status/activity";
import { formatAgo } from "@maxhq/shared/status/format";
import { type AgentKind, kindName, ownKind } from "@maxhq/shared/status/agent-kind";
import { type SessionPillLook, sessionPillLabel } from "@maxhq/shared/terminal/session";
import {
	ArrowRight,
	CircleAlert,
	File,
	FileText,
	Folder,
	Image,
	LoaderCircle,
	Quote,
	SquareTerminal,
	StickyNote,
} from "lucide-react";
import { type CSSProperties, type ReactNode, useState } from "react";

import { VendorIcon } from "@/agent-view/icons";
import { ActivityDot } from "@/components/status/dots";
import { formatBytes } from "@/composer/attachments";
import { cn } from "@/lib/utils";

import { GitIcon, type PillFamily, type PillState, gitFamily } from "./pill";

/** Each kind of pill. */
export const PILL_KINDS = [
	"attachment",
	"visual",
	"quote",
	"agent",
	"note",
	"git_item",
	"output",
	"session",
] as const;

export type PillKind = (typeof PILL_KINDS)[number];

/** What a kind can show beyond its pill. */
export type PillCapability = {
	readonly details: "none" | "popover";
};

export type PillDefinition = {
	readonly kind: PillKind;
	readonly capabilities: PillCapability;
};

const DEFINITIONS = [
	{ kind: "attachment", capabilities: { details: "popover" } },
	{ kind: "visual", capabilities: { details: "popover" } },
	{ kind: "quote", capabilities: { details: "popover" } },
	{ kind: "agent", capabilities: { details: "popover" } },
	{ kind: "note", capabilities: { details: "popover" } },
	{ kind: "git_item", capabilities: { details: "popover" } },
	{ kind: "output", capabilities: { details: "none" } },
	{ kind: "session", capabilities: { details: "popover" } },
] as const satisfies readonly PillDefinition[];

const buildDefinitions = (
	definitions: readonly PillDefinition[]
): ReadonlyMap<PillKind, PillDefinition> => {
	const registry = new Map<PillKind, PillDefinition>();
	for (const definition of definitions) {
		if (registry.has(definition.kind))
			throw new Error(`Duplicate pill definition: ${definition.kind}`);
		registry.set(definition.kind, definition);
	}
	for (const kind of PILL_KINDS)
		if (!registry.has(kind)) throw new Error(`Missing pill definition: ${kind}`);
	return registry;
};

const PILL_DEFINITIONS = buildDefinitions(DEFINITIONS);

const definitionOf = (kind: PillKind): PillDefinition => PILL_DEFINITIONS.get(kind)!;

/** How one kind draws its records. */
export type PillHandler<TRecord, TContext, TResult> = {
	readonly kind: PillKind;
	readonly canRender?: (record: TRecord, context: TContext) => boolean;
	readonly render: (record: TRecord, context: TContext, definition: PillDefinition) => TResult;
};

export type PillRegistry<TRecord, TContext, TResult> = {
	readonly definition: (kind: PillKind) => PillDefinition;
	readonly render: (kind: string, record: TRecord | undefined, context: TContext) => TResult;
};

/** A registry with one handler per kind. It throws on a kind with two, or one with none. */
export const createPillRegistry = <TRecord, TContext, TResult>(options: {
	readonly handlers: readonly PillHandler<TRecord, TContext, TResult>[];
	readonly requiredKinds?: readonly PillKind[];
	readonly fallback: (kind: string, record: TRecord | undefined, context: TContext) => TResult;
}): PillRegistry<TRecord, TContext, TResult> => {
	const handlers = new Map<PillKind, PillHandler<TRecord, TContext, TResult>>();
	for (const handler of options.handlers) {
		if (handlers.has(handler.kind)) throw new Error(`Duplicate pill handler: ${handler.kind}`);
		handlers.set(handler.kind, handler);
	}
	for (const kind of options.requiredKinds ?? [])
		if (!handlers.has(kind)) throw new Error(`Missing pill handler: ${kind}`);
	return {
		definition: definitionOf,
		render: (kind, record, context) => {
			const handler = handlers.get(kind as PillKind);
			if (!handler || record === undefined || handler.canRender?.(record, context) === false)
				return options.fallback(kind, record, context);
			return handler.render(record, context, definitionOf(handler.kind));
		},
	};
};

/** What a file's pill knows about the file. */
export type FileRecord = {
	readonly name: string;
	readonly kind: AttachmentKind | "missing";
	readonly bytes: number | null;
	readonly mime: string | null;
	/** Where it is, on this Mac or on the agent's device. */
	readonly path: string | null;
	/** A picture of it: the pasted image, a thumbnail or maxd's. */
	readonly image: string | null;
	/** Whether `image` is the whole picture, so the preview can say its size. */
	readonly fullImage: boolean;
	/** An image's or PDF's number in the message. */
	readonly number: number | undefined;
	readonly progress: FileProgress;
	/**
	 * What it is and its size, or how far it got, such as "Rust, 4 KB" or
	 * "0.8 of 1.2 MB". Left out, the preview gives the size.
	 */
	readonly detail?: string;
};

/** One piece of context, as the registry draws it. */
export type PillRecord =
	| { readonly kind: "attachment"; readonly file: FileRecord }
	| {
			readonly kind: "visual";
			readonly title: string;
			/** Where it came from, such as "Weekly report on Mac", or null when it's not known. */
			readonly from: string | null;
			/** The page's size, once it's known. */
			readonly bytes: number | null;
			readonly progress: FileProgress;
			/** The page drawn small, made only once the preview opens. */
			readonly page?: ReactNode;
	  }
	| {
			readonly kind: "quote";
			readonly ref: QuoteRef;
			/** Whether the quoted agent is gone, such as in a message sent before it was removed. */
			readonly gone?: boolean;
			/** Goes to the passage. Set, the preview offers Jump to. */
			readonly onJump?: () => void;
	  }
	| {
			readonly kind: "agent";
			readonly ref: AgentRef;
			/** The agent now, or null once it's gone. */
			readonly agent: Agent | null;
			readonly deviceName: string | null;
			/** What the agent shows as, from its account, so a local agent reads Local. */
			readonly agentKind?: AgentKind;
	  }
	| {
			readonly kind: "note";
			readonly ref: NoteRef;
			/** The note now, or null once it's deleted or no device has it. */
			readonly note: NoteSummary | null;
			readonly projectName: string | null;
			/** Who saved its newest version, in words. */
			readonly savedBy: string | null;
			/** Its first lines, drawn only once the preview opens, since they're read then. */
			readonly lines?: ReactNode;
	  }
	| { readonly kind: "git_item"; readonly ref: GitItemRef }
	| {
			readonly kind: "output";
			/** The output as it stands now, or as it went in a sent message. */
			readonly ref: OutputRef;
			/** What goes to the agent, made only once the preview opens. */
			readonly preview?: ReactNode;
	  }
	| {
			readonly kind: "session";
			readonly ref: SessionRef;
			/** How its card's commands stand, or null once the card is gone. */
			readonly look: SessionPillLook | null;
			/** Its card's commands, with ticks while you may change them. */
			readonly preview?: ReactNode;
	  };

export type PillContext = { readonly now: Timestamp };

/** How a pill looks, what it says and what its preview shows. */
export type PillPresentation = {
	readonly family: PillFamily;
	readonly state: PillState;
	readonly progress: number | null;
	readonly icon: ReactNode;
	readonly label: string;
	readonly aria: string;
	readonly preview: ReactNode | null;
	/** What follows the words, such as a command's exit code or its spinner. */
	readonly trail?: ReactNode;
};

const ICON_CLASS = "size-[1em] shrink-0";

const fileIcon = (file: FileRecord): ReactNode => {
	if (file.progress.state === "failed") return <CircleAlert aria-hidden className={ICON_CLASS} />;
	if (file.progress.state === "staging")
		return <LoaderCircle aria-hidden className={cn(ICON_CLASS, "pill-spin")} />;
	if (file.kind === "image" && file.image)
		return <img alt="" src={file.image} className="pill-thumb" draggable={false} />;
	switch (file.kind) {
		case "image":
			return <Image aria-hidden className={ICON_CLASS} />;
		case "pdf":
			return <FileText aria-hidden className={ICON_CLASS} />;
		case "folder":
			return <Folder aria-hidden className={ICON_CLASS} />;
		default:
			return <File aria-hidden className={ICON_CLASS} />;
	}
};

const fileState = (file: FileRecord): PillState => {
	if (file.kind === "missing") return "unresolved";
	switch (file.progress.state) {
		case "ready":
			return "ready";
		case "staging":
			return "staging";
		case "uploading":
			return "uploading";
		case "failed":
			return "failed";
	}
};

/** An image's full size, read once the preview loads. */
const ImagePreview = ({ file }: { file: FileRecord }) => {
	useI18n();

	const [size, setSize] = useState<{ width: number; height: number } | null>(null);
	if (!file.image) return null;
	return (
		<>
			<img
				alt=""
				src={file.image}
				className="mb-2 block max-h-80 max-w-80 rounded-md border border-border object-contain"
				onLoad={(event) => {
					if (!file.fullImage) return;
					const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
					if (width > 0 && height > 0) setSize({ width, height });
				}}
			/>
			{size && (
				<div className="text-muted-foreground">
					{formatNumber(size.width)} × {formatNumber(size.height)}
				</div>
			)}
		</>
	);
};

const FileDetails = ({ file }: { file: FileRecord }) => {
	useI18n();
	return (
		<div className="max-w-80 space-y-1 text-left">
			{file.kind === "image" && <ImagePreview file={file} />}
			<div className="font-medium break-words text-foreground">{file.name}</div>
			{file.path && file.path !== file.name && (
				<code className="block font-mono text-[11px] break-all text-muted-foreground">
					{file.path}
				</code>
			)}
			{file.detail !== undefined ? (
				<div className="text-muted-foreground">{file.detail}</div>
			) : (
				file.bytes !== null && (
					<div className="text-muted-foreground">{formatBytes(file.bytes)}</div>
				)
			)}
			{file.progress.state === "failed" && (
				<div className="text-status-error-text">{file.progress.error}</div>
			)}
		</div>
	);
};

/** What a visual's preview says under the page about where it goes. */
export const visualFooter = (): string => t("Sent as a file the agent can read and change.");

const VisualDetails = ({ record }: { record: Extract<PillRecord, { kind: "visual" }> }) => {
	const { t } = useI18n();
	return (
		<div className="max-w-80 space-y-1 text-left">
			{record.page}
			<div className="font-medium break-words text-foreground">
				{record.title || t("Visual")}
			</div>
			{(record.from !== null || record.bytes !== null) && (
				<div className="text-muted-foreground">
					{[
						record.from === null ? null : t("From {{from}}", { from: record.from }),
						record.bytes === null ? null : formatBytes(record.bytes),
					]
						.filter(Boolean)
						.join(", ")}
				</div>
			)}
			{record.progress.state === "failed" && (
				<div className="text-status-error-text">{record.progress.error}</div>
			)}
			<div className="text-[11px] text-muted-foreground">{visualFooter()}</div>
		</div>
	);
};

/** The diamond a visual's pill and card show. */
export const VisualMark = ({ className }: { className?: string }) => (
	<span aria-hidden className={cn("leading-none", className)}>
		◆
	</span>
);

const QUOTE_SOURCES: Record<QuoteRef["source"], string> = {
	get reply() {
		return t("From a reply");
	},
	get request() {
		return t("From a message you sent");
	},
	get question() {
		return t("From a question");
	},
	get output() {
		return t("From the output");
	},
	get other() {
		return t("Quoted");
	},
};

const QuoteDetails = ({
	quote,
	gone,
	onJump,
}: {
	quote: QuoteRef;
	gone: boolean;
	onJump?: () => void;
}) => {
	const { t } = useI18n();
	return (
		<div className="max-w-80 space-y-1.5 text-left">
			<blockquote className="max-h-40 overflow-y-auto border-l-2 border-(--pill-quote-border) pl-2 whitespace-pre-wrap text-foreground">
				{quote.text}
			</blockquote>
			<div className="text-muted-foreground">
				{quote.source === "output" && quote.from
					? t("Output of {{from}}", { from: quote.from })
					: QUOTE_SOURCES[quote.source]}
				, {quote.agent_title}
			</div>
			{gone && <div className="text-muted-foreground">{t("This agent is gone")}</div>}
			{onJump && !gone && (
				<button
					type="button"
					className="h-6 rounded-[5px] border border-input bg-secondary px-2 text-[11.5px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
					onClick={onJump}
				>
					{t("Jump to")}
				</button>
			)}
		</div>
	);
};

/** What an agent pill's agent shows as: the kind it was given, else from its own record. */
const pillKind = (record: Extract<PillRecord, { kind: "agent" }>): AgentKind =>
	record.agentKind ?? (record.agent ? ownKind(record.agent) : record.ref.vendor);

const AgentDetails = ({
	record,
	now,
}: {
	record: Extract<PillRecord, { kind: "agent" }>;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { ref, agent, deviceName } = record;
	const look = agent ? agentLook(agent, now) : null;
	return (
		<div className="max-w-80 space-y-1 text-left">
			<div className="font-medium break-words text-foreground">
				{agent?.title ?? ref.title}
			</div>
			<div className="text-muted-foreground">
				{[kindName(pillKind(record)), deviceName].filter(Boolean).join(t(" on "))}
			</div>
			{look ? (
				<div className="flex items-center gap-1.5 text-muted-foreground">
					<ActivityDot look={look} decorative />
					{ACTIVITY_LABEL[look]}
					{agent?.last_activity_at != null &&
						`, ${formatAgo(agent.last_activity_at, now)}`}
				</div>
			) : (
				<div className="text-muted-foreground">{t("This agent is gone")}</div>
			)}
			<div className="text-muted-foreground">{t("The agent can read its conversation")}</div>
		</div>
	);
};

const NoteDetails = ({
	record,
	now,
}: {
	record: Extract<PillRecord, { kind: "note" }>;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { ref, note, projectName, savedBy, lines } = record;
	return (
		<div className="max-w-80 space-y-1 text-left">
			<div className="font-medium break-words text-foreground">
				{noteLabel(note?.title ?? ref.title)}
			</div>
			{projectName !== null && (
				<div className="text-muted-foreground">
					{t("Note in {{projectName}}", { projectName: projectName })}
				</div>
			)}
			{note ? (
				<div className="text-muted-foreground">
					{t("Changed {{value2}}{{value4}}", {
						value2: formatAgo(note.at, now),
						value4:
							(savedBy !== null && t(" by {{savedBy}}", { savedBy: savedBy })) || "",
					})}
				</div>
			) : (
				<div className="text-muted-foreground">{t("This note is gone")}</div>
			)}
			{note && lines}
		</div>
	);
};

/** A pull request's or issue's number, state, title and branches. */
export const GitItemDetails = ({ item, now }: { item: GitItemRef; now: Timestamp }) => {
	const { t } = useI18n();

	return (
		<div
			className="max-w-80 space-y-1 overflow-hidden text-left"
			style={{ "--pill-text": `var(--pill-${gitFamily(item.state)}-text)` } as CSSProperties}
		>
			<div className="flex items-center gap-1.5 font-medium">
				<GitIcon
					kind={item.kind}
					state={item.state}
					aria-hidden
					className="size-3.5 shrink-0 text-(--pill-text)"
				/>
				<span className="text-foreground">
					{gitKindWords(item)} #{formatNumber(item.number, { useGrouping: false })}
				</span>
				<span className="text-(--pill-text)">{gitStateWords(item.state)}</span>
			</div>
			<div className="break-words text-foreground">{item.title}</div>
			{item.head && item.base && (
				<div className="flex min-w-0 items-center gap-1 text-[11px] text-muted-foreground">
					<code className="truncate">{item.head}</code>
					<ArrowRight aria-hidden className="size-3 shrink-0" />
					<code className="truncate">{item.base}</code>
				</div>
			)}
			<div className="text-[11px] text-muted-foreground">
				{t("{{repo}}, checked {{value4}}", {
					repo: item.repo,
					value4: formatAgo(item.checked_at, now),
				})}
			</div>
		</div>
	);
};

/** Whether the command ended with an exit code other than 0. */
export const outputFailed = (ref: OutputRef): boolean =>
	!ref.running && !ref.lines && ref.exit_code !== null && ref.exit_code !== 0;

/** What an output's pill says after the command: the lines picked from it, or how it ended. */
export const outputState = (ref: OutputRef): string | null => {
	if (ref.lines) return t("lines {{first}}-{{last}}", ref.lines);
	if (ref.running) return t("running");
	return ref.exit_code === null ? null : t("exit {{code}}", { code: ref.exit_code });
};

const percentOf = (progress: FileProgress): number | null =>
	progress.state === "uploading" ? progress.percent : null;

/** Every kind's look. */
export const pillRegistry = createPillRegistry<PillRecord, PillContext, PillPresentation>({
	requiredKinds: PILL_KINDS,
	handlers: [
		{
			kind: "attachment",
			canRender: (record) => record.kind === "attachment",
			render: (record) => {
				const { file } = record as Extract<PillRecord, { kind: "attachment" }>;
				const percent = percentOf(file.progress);
				return {
					family: "file",
					state: fileState(file),
					progress:
						file.progress.state === "uploading"
							? (percent ?? 0) / 100
							: file.progress.state === "staging"
								? 0
								: null,
					icon: fileIcon(file),
					label: middleTruncate(fileLabel(file, file.number)),
					aria: fileAria(
						file,
						file.number,
						file.bytes === null ? null : formatBytes(file.bytes),
						file.progress
					),
					preview: <FileDetails file={file} />,
				};
			},
		},
		{
			kind: "visual",
			canRender: (record) => record.kind === "visual",
			render: (record) => {
				const visual = record as Extract<PillRecord, { kind: "visual" }>;
				const { progress } = visual;
				const title = visual.title || t("Visual");
				const percent = percentOf(progress);
				return {
					family: "visual",
					state: progress.state,
					progress:
						progress.state === "uploading"
							? (percent ?? 0) / 100
							: progress.state === "staging"
								? 0
								: null,
					icon:
						progress.state === "failed" ? (
							<CircleAlert aria-hidden className={ICON_CLASS} />
						) : progress.state === "staging" ? (
							<LoaderCircle aria-hidden className={cn(ICON_CLASS, "pill-spin")} />
						) : (
							<VisualMark className={ICON_CLASS} />
						),
					label: middleTruncate(title),
					aria: [
						t("Visual {{title}}", { title }),
						visual.from,
						visual.bytes === null ? null : formatBytes(visual.bytes),
						progress.state === "failed" ? t("failed") : null,
					]
						.filter(Boolean)
						.join(", "),
					preview: <VisualDetails record={visual} />,
				};
			},
		},
		{
			kind: "quote",
			canRender: (record) => record.kind === "quote",
			render: (record) => {
				const {
					ref,
					gone = false,
					onJump,
				} = record as Extract<PillRecord, { kind: "quote" }>;
				const label = quoteLabel(ref.text);
				return {
					family: "quote",
					state: gone ? "unresolved" : "ready",
					progress: null,
					icon: <Quote aria-hidden className={ICON_CLASS} />,
					label,
					aria: t("Quote from {{agent_title}}, {{label}}{{value3}}", {
						agent_title: ref.agent_title,
						label: label,
						value3: gone ? t(", gone") : "",
					}),
					preview: <QuoteDetails quote={ref} gone={gone} onJump={onJump} />,
				};
			},
		},
		{
			kind: "agent",
			canRender: (record) => record.kind === "agent",
			render: (record, { now }) => {
				const agent = record as Extract<PillRecord, { kind: "agent" }>;
				const title = agent.agent?.title ?? agent.ref.title;
				const look = agent.agent ? agentLook(agent.agent, now) : null;
				return {
					family: "agent",
					state: agent.agent ? "ready" : "unresolved",
					progress: null,
					icon: <VendorIcon kind={pillKind(agent)} size={12} />,
					label: middleTruncate(title),
					aria: [
						t("Agent {{title}}", { title: title }),
						kindName(pillKind(agent)),
						agent.deviceName,
						look ? ACTIVITY_LABEL[look].toLowerCase() : "gone",
					]
						.filter(Boolean)
						.join(", "),
					preview: <AgentDetails record={agent} now={now} />,
				};
			},
		},
		{
			kind: "note",
			canRender: (record) => record.kind === "note",
			render: (record, { now }) => {
				const note = record as Extract<PillRecord, { kind: "note" }>;
				const title = noteLabel(note.note?.title ?? note.ref.title);
				return {
					family: "note",
					state: note.note ? "ready" : "unresolved",
					progress: null,
					icon: <StickyNote aria-hidden className={ICON_CLASS} />,
					label: middleTruncate(title),
					aria: [
						t("Note {{title}}", { title: title }),
						note.projectName,
						note.note ? null : "gone",
					]
						.filter(Boolean)
						.join(", "),
					preview: <NoteDetails record={note} now={now} />,
				};
			},
		},
		{
			kind: "git_item",
			canRender: (record) => record.kind === "git_item",
			render: (record, { now }) => {
				const { ref } = record as Extract<PillRecord, { kind: "git_item" }>;
				return {
					family: gitFamily(ref.state),
					state: "ready",
					progress: null,
					icon: (
						<GitIcon
							kind={ref.kind}
							state={ref.state}
							aria-hidden
							className={ICON_CLASS}
						/>
					),
					label: middleTruncate(gitItemLabel(ref)),
					aria: `${gitKindWords(ref)} ${ref.number}, ${ref.title}, ${gitStateWords(ref.state)}`,
					preview: <GitItemDetails item={ref} now={now} />,
				};
			},
		},
		{
			kind: "output",
			canRender: (record) => record.kind === "output",
			render: (record) => {
				const { ref, preview } = record as Extract<PillRecord, { kind: "output" }>;
				const state = outputState(ref);
				return {
					family: outputFailed(ref) ? "output-failed" : "output",
					state: "ready",
					progress: null,
					icon: <SquareTerminal aria-hidden className={ICON_CLASS} />,
					label: middleTruncate(oneLine(ref.command)),
					trail:
						ref.running && !ref.lines ? (
							<LoaderCircle aria-hidden className={cn(ICON_CLASS, "pill-spin")} />
						) : state === null ? null : (
							<span className="pill-trail">{state}</span>
						),
					aria: [t("Output of {{from}}", { from: oneLine(ref.command) }), state]
						.filter(Boolean)
						.join(", "),
					preview: preview ?? null,
				};
			},
		},
		{
			kind: "session",
			canRender: (record) => record.kind === "session",
			render: (record) => {
				const { ref, look, preview } = record as Extract<PillRecord, { kind: "session" }>;
				const label = look ? sessionPillLabel(look) : t("Shell session");
				return {
					family: look?.tone === "failed" ? "output-failed" : "output",
					state: look ? "ready" : "unresolved",
					progress: null,
					icon: <SquareTerminal aria-hidden className={ICON_CLASS} />,
					label,
					trail: look?.running ? (
						<LoaderCircle aria-hidden className={cn(ICON_CLASS, "pill-spin")} />
					) : null,
					aria: [
						t("{{label}} on {{device}}", { label, device: ref.device_name }),
						look?.running ? t("running") : null,
					]
						.filter(Boolean)
						.join(", "),
					preview: preview ?? null,
				};
			},
		},
	],
	fallback: (kind) => ({
		family: "file",
		state: "unresolved",
		progress: null,
		icon: <File aria-hidden className={ICON_CLASS} />,
		label: kind,
		aria: t("Unknown {{kind}}", { kind: kind }),
		preview: null,
	}),
});

/** A record's look. */
export const presentPill = (record: PillRecord, context: PillContext): PillPresentation =>
	pillRegistry.render(record.kind, record, context);
