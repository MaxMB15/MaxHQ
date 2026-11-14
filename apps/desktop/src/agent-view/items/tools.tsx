// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The tool steps: commands, file edits, reads, searches, the web and any
// other tool. Each is one row, and commands, edits and tool calls open to
// show what they printed or changed. A file it read or wrote opens in an
// editor from its row, at the line, and for an agent on another device it
// can be downloaded from there too. The pictures a read
// or a tool showed the agent, such as a screenshot, sit under its row.

import {
	Bot,
	ChevronUp,
	Diamond,
	Eye,
	FileMinus,
	FilePlus,
	FileSymlink,
	FileText,
	Globe,
	Pencil,
	Search,
	Terminal,
	Wrench,
} from "lucide-react";
import type { ReactNode } from "react";

import { agentToolLine, agentToolOf, viewNames } from "@maxhq/shared/agent-view/agent-tools";
import { EDIT_VERBS, TOOL_VERBS, verbFor, WEB_VERBS } from "@maxhq/shared/agent-view/tool-line";
import type { AttachmentRef, FileEdit, ItemStatus } from "@maxhq/shared/bindings";
import { plural } from "@maxhq/shared/status/format";
import { editPath, shownCounts } from "@maxhq/shared/timeline/diff";

import { useView } from "@/app/services";
import { useRow } from "@maxhq/shared/agent-view/context";
import { DiffView } from "../diff-view";
import { DownloadButton } from "../download";
import {
	baseName,
	endedEarly as endedWord,
	folderOf,
	formatTook,
	isOpen,
	lineCount,
	lineSpan,
	tookOf,
} from "@maxhq/shared/agent-view/format";
import { useFold } from "@maxhq/shared/agent-view/find-context";
import { useKept } from "@maxhq/shared/agent-view/kept";
import { editLine, OpenFileButton, readLine } from "../open-in";
import { PictureSet } from "../picture-set";
import { PictureThumb } from "../picture-viewer";
import { Detail, Target, ToolRow } from "./tool-row";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const ICON = { size: 14, strokeWidth: 1.9, "aria-hidden": true } as const;

// What a step that didn't finish says at the end of its row.
const endedEarly = (status: ItemStatus): ReactNode => {
	const word = endedWord(status);
	return word === "Failed" ? <span className="text-status-error-text">{word}</span> : word;
};

const TAIL = 12;

// A command's output, with only its last lines until you ask for all of
// them. While find's active match is in the item, it shows all of them,
// under the whole command when that's given.
const Output = ({
	id,
	command,
	text,
	truncated,
}: {
	id: string;
	command?: string;
	text: string;
	truncated: boolean;
}) => {
	useI18n();
	const [all, setAll] = useFold(`all:${id}`, id);
	const lines = text.replace(/\n$/, "").split("\n");
	const hidden = all ? 0 : Math.max(0, lines.length - TAIL);
	return (
		<Detail>
			{command !== undefined ? (
				<pre
					data-find="plain"
					className="border-b border-secondary px-2.5 pt-1.5 pb-[7px] font-mono text-[11.5px] leading-[19px] break-words whitespace-pre-wrap text-terminal"
				>
					{command}
				</pre>
			) : null}
			{hidden > 0 ? (
				<button
					type="button"
					onClick={() => setAll(true)}
					className="flex h-6 w-full cursor-pointer items-center gap-1.5 border-b border-secondary px-2.5 text-[11.5px] text-muted-foreground hover:bg-accent"
				>
					<ChevronUp size={10} strokeWidth={2.4} aria-hidden />
					{translate("Show ")}
					{plural(hidden, "earlier line")}
				</button>
			) : null}
			{text === "" ? (
				<p className="px-2.5 py-1.5 text-[11.5px] text-muted-foreground">
					{translate("No output")}
				</p>
			) : (
				<pre
					data-find="plain"
					className="overflow-x-auto px-2.5 pt-1.5 pb-[7px] font-mono text-[11.5px] leading-[19px] text-secondary-foreground"
				>
					{lines.slice(hidden).join("\n")}
				</pre>
			)}
			{truncated ? (
				<p className="border-t border-secondary px-2.5 py-1 text-[11px] text-subtle-foreground">
					{translate("MaxHQ kept only part of the output.")}
				</p>
			) : null}
		</Detail>
	);
};

/** A shell command, with its output when opened. */
export const CommandRow = ({ item }: { item: ItemOf<"command"> }) => {
	useI18n();
	const [open, setOpen, held] = useFold(`open:${item.id}`, item.id);
	// The row's line shows the command's first line, cut to fit. While find's
	// active match is in the item, the details show the whole command, and
	// find counts that instead.
	const whole = open && held;
	const took = tookOf(item);
	const running = isOpen(item.status) && item.status !== "waiting";
	const failed = item.exit_code !== null && item.exit_code !== 0;
	const exit =
		item.exit_code !== null ? (
			<span className={failed ? "text-status-error-text" : undefined}>
				{took !== null
					? translate("Exit {{exit_code}}, {{took}}", {
							exit_code: item.exit_code,
							took: formatTook(took),
						})
					: translate("Exit {{exit_code}}", { exit_code: item.exit_code })}
			</span>
		) : null;
	const meta =
		endedEarly(item.status) ??
		exit ??
		(item.background
			? translate("In the background")
			: item.status === "waiting"
				? translate("Waiting for you")
				: null);
	return (
		<ToolRow
			icon={<Terminal {...ICON} />}
			verb={verbFor(item.status, TOOL_VERBS.command)}
			target={
				<Target title={item.cwd ?? undefined} find={!whole}>
					{item.command.split("\n")[0]}
				</Target>
			}
			meta={meta}
			since={item.started_at}
			running={running}
			open={open}
			onToggle={() => setOpen(!open)}
		>
			<Output
				id={item.id}
				command={whole ? item.command : undefined}
				text={item.output}
				truncated={item.output_truncated}
			/>
		</ToolRow>
	);
};

const EDIT_ICON: Record<FileEdit["op"], ReactNode> = {
	add: <FilePlus {...ICON} />,
	modify: <Pencil {...ICON} />,
	delete: <FileMinus {...ICON} />,
	rename: <FileSymlink {...ICON} />,
};

/** Lines added and removed, such as "+3 −1". */
export const Counts = ({ edit }: { edit: Pick<FileEdit, "additions" | "deletions"> }) => {
	useI18n();
	const { additions, deletions } = shownCounts(edit.additions, edit.deletions);
	return (
		<span className="inline-flex gap-1.5">
			{additions !== null ? <span className="text-status-ready">+{additions}</span> : null}
			{deletions !== null ? <span className="text-destructive">−{deletions}</span> : null}
		</span>
	);
};

const FileEditRow = ({
	id,
	status,
	edit,
	since,
}: {
	id: string;
	status: ItemStatus;
	edit: FileEdit;
	since: number | null;
}) => {
	useI18n();
	const [open, setOpen] = useKept(`open:${id}`, false);
	const { remote } = useRow();
	return (
		<ToolRow
			icon={EDIT_ICON[edit.op]}
			verb={verbFor(status, EDIT_VERBS[edit.op])}
			target={<Target>{editPath(edit)}</Target>}
			meta={endedEarly(status) ?? <Counts edit={edit} />}
			since={since}
			running={status === "running" || status === "pending"}
			open={open}
			onToggle={() => setOpen(!open)}
			action={
				status === "completed" && edit.op !== "delete" ? (
					<>
						<OpenFileButton path={edit.path} line={editLine(edit.patch)} />
						{remote ? <DownloadButton path={edit.path} /> : null}
					</>
				) : undefined
			}
		>
			<Detail>
				{edit.patch ? (
					<DiffView patch={edit.patch} />
				) : (
					<p className="px-2.5 py-1.5 text-[11.5px] text-muted-foreground">
						{translate("The CLI didn't send a diff for this file.")}
					</p>
				)}
			</Detail>
		</ToolRow>
	);
};

/** Edits to files, one row each. */
export const FileChangeRow = ({ item }: { item: ItemOf<"file_change"> }) => {
	useI18n();
	return (
		<div className="flex flex-col gap-px">
			{item.changes.map((edit, i) => (
				<FileEditRow
					// Edits have no ID of their own; their place in the item never changes.
					key={i}
					id={`${item.id}:${i}`}
					status={item.status}
					edit={edit}
					since={item.started_at}
				/>
			))}
		</div>
	);
};

// A step's row with the pictures it showed the agent under it.
const WithPictures = ({
	images = [],
	children,
}: {
	images?: readonly AttachmentRef[];
	children: ReactNode;
}) => {
	useI18n();
	return images.length === 0 ? (
		children
	) : (
		<div className="flex flex-col gap-1">
			{children}
			<PictureSet>
				<div className="ml-7 flex flex-wrap gap-2">
					{images.map((file) => (
						<PictureThumb key={file.id} file={file} />
					))}
				</div>
			</PictureSet>
		</div>
	);
};

/** A file the agent read. */
export const FileReadRow = ({ item }: { item: ItemOf<"file_read"> }) => {
	useI18n();
	const { remote } = useRow();
	return (
		<WithPictures images={item.images}>
			<ToolRow
				icon={<FileText {...ICON} />}
				verb={verbFor(item.status, TOOL_VERBS.read)}
				target={<Target>{item.path}</Target>}
				meta={endedEarly(item.status) ?? lineSpan(item.lines)}
				since={item.started_at}
				running={isOpen(item.status)}
				action={
					item.status === "completed" ? (
						<>
							<OpenFileButton path={item.path} line={readLine(item.lines)} />
							{remote ? <DownloadButton path={item.path} /> : null}
						</>
					) : undefined
				}
			/>
		</WithPictures>
	);
};

/**
 * A file the agent opened in MaxHQ's viewer for you. The row names the file,
 * its folder and the line, and its buttons show it again or open it in an
 * editor. One from outside the agent's folders asks before it shows.
 */
export const OpenedFileRow = ({ item }: { item: ItemOf<"agent_opened_file"> }) => {
	useI18n();
	const line = item.line ?? undefined;
	return (
		<ToolRow
			icon={<Eye {...ICON} />}
			verb={translate("Opened")}
			target={
				<>
					<Target title={item.path}>{baseName(item.path)}</Target>
					<span className="shrink-0">{translate("in")}</span>
					<Target className="max-w-[50%] shrink" title={folderOf(item.path)}>
						{folderOf(item.path)}
					</Target>
				</>
			}
			meta={line !== undefined ? translate("Line {{line}}", { line: line }) : undefined}
			action={<OpenFileButton path={item.path} line={line} />}
		/>
	);
};

/**
 * A web page the agent showed you, such as a chart, while its page is read
 * or when it can't be. The row names it, and its button opens the page's
 * source in MaxHQ's viewer.
 */
export const VisualRow = ({
	item,
	note,
}: {
	item: ItemOf<"agent_visual">;
	/** Why the page couldn't be read, in place of the row's own note. */
	note?: string;
}) => {
	useI18n();
	return (
		<ToolRow
			icon={<Diamond {...ICON} className="fill-current text-(--pill-visual-text)" />}
			verb={translate("Showed")}
			target={
				<Target title={item.attachment.name}>
					{item.title.trim() || translate("a visual")}
				</Target>
			}
			meta={note ?? (item.replaces ? translate("New version") : undefined)}
			action={<OpenFileButton path={item.attachment.path} />}
		/>
	);
};

/** A search through files. */
export const SearchRow = ({ item }: { item: ItemOf<"search"> }) => {
	useI18n();
	return (
		<ToolRow
			icon={<Search {...ICON} />}
			verb={verbFor(item.status, TOOL_VERBS.search)}
			target={
				<>
					<Target>{item.query}</Target>
					{item.scope ? (
						<>
							<span className="shrink-0">{translate("in")}</span>
							<Target className="max-w-[40%]">{item.scope}</Target>
						</>
					) : null}
				</>
			}
			meta={
				endedEarly(item.status) ??
				(item.matches === null
					? null
					: item.matches === 0
						? translate("No matches")
						: plural(item.matches, "match", "matches"))
			}
			since={item.started_at}
			running={isOpen(item.status)}
		/>
	);
};

/** A web search or a page fetched. */
export const WebRow = ({ item }: { item: ItemOf<"web"> }) => {
	useI18n();
	return (
		<ToolRow
			icon={<Globe {...ICON} />}
			verb={verbFor(item.status, WEB_VERBS[item.action])}
			target={<Target>{item.target}</Target>}
			meta={endedEarly(item.status)}
			since={item.started_at}
			running={isOpen(item.status)}
		/>
	);
};

const Section = ({ label, text }: { label: string; text: string }) => {
	useI18n();
	return (
		<div className="flex flex-col border-b border-secondary last:border-b-0">
			<span className="px-2.5 pt-1.5 text-[11px] text-subtle-foreground">{label}</span>
			<pre
				data-find="plain"
				className="max-h-[320px] overflow-auto px-2.5 pt-0.5 pb-1.5 font-mono text-[11.5px] leading-[19px] break-words whitespace-pre-wrap text-secondary-foreground"
			>
				{text}
			</pre>
		</div>
	);
};

/** Any other tool, such as one from an MCP server. */
// A call to one of MaxHQ's agent tools, in words: "Started Fix the login
// bug on devbox" or "Waiting on 2 agents".
const AgentToolCallRow = ({ item }: { item: ItemOf<"tool_call"> }) => {
	useI18n();
	const view = useView((s) => s.view);
	const line = agentToolLine(item, viewNames(view));
	return line ? (
		<CallRow
			item={item}
			icon={<Bot {...ICON} />}
			verb={verbFor(item.status, line)}
			target={<Target>{line.target}</Target>}
			note={line.note}
		/>
	) : (
		<CallRow item={item} />
	);
};

export const ToolCallRow = ({ item }: { item: ItemOf<"tool_call"> }) => {
	useI18n();
	return agentToolOf(item.server, item.name) ? (
		<AgentToolCallRow item={item} />
	) : (
		<CallRow item={item} />
	);
};

const CallRow = ({
	item,
	icon,
	verb,
	target,
	note,
}: {
	item: ItemOf<"tool_call">;
	icon?: ReactNode;
	verb?: string;
	target?: ReactNode;
	note?: string | null;
}) => {
	useI18n();
	const [open, setOpen] = useFold(`open:${item.id}`, item.id);
	const name = item.server ? `${item.server}:${item.name}` : item.name;
	const hasDetail = Boolean(item.input || item.output);
	const took = tookOf(item);
	return (
		<WithPictures images={item.images}>
			<ToolRow
				icon={icon ?? <Wrench {...ICON} />}
				verb={verb ?? verbFor(item.status, TOOL_VERBS.call)}
				target={
					target ?? (
						<>
							<Target className="shrink-0 max-w-[45%]">{name}</Target>
							{item.title ? (
								<span className="min-w-0 truncate">{item.title}</span>
							) : null}
						</>
					)
				}
				meta={
					endedEarly(item.status) ??
					note ??
					(took !== null && took >= 1000 ? formatTook(took) : null)
				}
				since={item.started_at}
				running={isOpen(item.status)}
				open={hasDetail ? open : undefined}
				onToggle={hasDetail ? () => setOpen(!open) : undefined}
			>
				<Detail>
					{item.input ? <Section label={translate("Input")} text={item.input} /> : null}
					{item.output ? (
						<Section
							label={
								lineCount(item.output) > 1
									? translate("Output, {{plural}}", {
											plural: plural(lineCount(item.output), "line"),
										})
									: translate("Output")
							}
							text={item.output}
						/>
					) : null}
				</Detail>
			</ToolRow>
		</WithPictures>
	);
};
