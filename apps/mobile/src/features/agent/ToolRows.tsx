// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The tool steps, one line each as on the desktop: an icon, a verb, what it
// acted on and a figure at the end. Commands, edits and tool calls open under
// their line to show what they printed or changed, and the pictures a read or
// a tool showed the agent sit under its line. A file read, edited or opened
// shows in the file viewer, at the line the desktop opens it at: a tap on
// the row for a read or an opened file, and the eye at the end of an edit's
// row, whose own tap opens its changes. A claude.ai page a Claude agent
// published or read opens in the browser from the arrow at its row's end.
// A command you ran with `!` gets a row too, saying whether it waits for you
// or how it ended, with its output once it ended.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import type { ComponentType, ReactNode } from "react";
import { useMemo } from "react";
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import { agentToolLine, agentToolOf, viewNames } from "@maxhq/shared/agent-view/agent-tools";
import {
	artifactLine,
	type ArtifactLine,
	type ArtifactTool,
	claudeLink,
} from "@maxhq/shared/agent-view/artifact-line";
import { useFold } from "@maxhq/shared/agent-view/find-context";
import {
	endedEarly as endedWord,
	formatTook,
	isOpen,
	lineCount,
	lineSpan,
	tookOf,
} from "@maxhq/shared/agent-view/format";
import { nameOf, parentOf } from "@maxhq/shared/folders/path";
import { useKept } from "@maxhq/shared/agent-view/kept";
import { EDIT_VERBS, TOOL_VERBS, verbFor, WEB_VERBS } from "@maxhq/shared/agent-view/tool-line";
import { editLine, readLine } from "@maxhq/shared/agent-view/viewer-files";
import type {
	AttachmentRef,
	FileEdit,
	ItemStatus,
	TimelineItem,
	Timestamp,
} from "@maxhq/shared/bindings";

import { endedText } from "@maxhq/shared/terminal/ended";
import { handoffOf, handoffPhoneText } from "@maxhq/shared/terminal/handoff";
import { runStatus } from "@maxhq/shared/terminal/output";
import { terminalById, waitsForYou } from "@maxhq/shared/terminal/select";
import { editPath } from "@maxhq/shared/timeline/diff";
import { VisualOriginContext } from "@maxhq/shared/visuals/origin";

import type { Screens } from "../../navigation";
import { useView } from "../../services";
import { color, font, radius, size, TOUCH } from "../../theme";
import { AnsiText } from "../../ui/AnsiText";
import { Spinner } from "../../ui/Dot";
import { Elapsed } from "../../ui/Elapsed";
import {
	AppWindow,
	BadgeCheck,
	Bot,
	ChevronDown,
	ChevronRight,
	ChevronUp,
	Database,
	ExternalLink,
	Eye,
	FileMinus,
	FilePlus,
	FileSymlink,
	FileText,
	Globe,
	MessageSquare,
	Pencil,
	Search,
	Terminal,
	Wrench,
} from "../../ui/glyphs";
import { PictureThumbs } from "../../ui/Picture";
import { fileRoute } from "../viewer/file";
import { useFeedOrigin } from "../visuals/feed-origin";
import { visualOfItem } from "../visuals/visual";
import { VisualOpenRow } from "../visuals/VisualOpenRow";
import { useFeed } from "./feed-context";
import { Counts, PatchView } from "./PatchView";

type ItemOf<K extends TimelineItem["kind"]> = Extract<TimelineItem, { kind: K }>;

type Glyph = ComponentType<{ size?: number; strokeWidth?: number; color?: string }>;

const Icon = ({ glyph: Shown, tint }: { glyph: Glyph; tint?: string }) => (
	<Shown size={14} strokeWidth={1.9} color={tint ?? color.mutedForeground} />
);

// The word at the end of a step that didn't finish.
const endedEarly = (status: ItemStatus): ReactNode => {
	const word = endedWord(status);
	return status === "failed" ? <Text style={styles.failed}>{word}</Text> : word;
};

/** A path, a query or a command in a row, cut short to fit. A path keeps its end, the file's name. */
const Target = ({
	children,
	path = false,
	most,
}: {
	children: string;
	path?: boolean;
	/** The share of the row it may take, for one that sits beside other text. */
	most?: `${number}%`;
}) => {
	return (
		<Text
			numberOfLines={1}
			ellipsizeMode={path ? "middle" : "tail"}
			style={[styles.target, most ? { maxWidth: most, flexShrink: 0 } : null]}
		>
			{children}
		</Text>
	);
};

/** The box a row's details open in, under the verb. */
const Detail = ({ children }: { children: ReactNode }) => {
	return <View style={styles.detail}>{children}</View>;
};

/**
 * Showing a file in the viewer, or a page in the browser, and what
 * VoiceOver calls it. The eye marks it unless it names its own glyph.
 */
type ShowFile = { readonly label: string; readonly onPress: () => void; readonly glyph?: Glyph };

// The rows are shorter than a finger, so the eye takes taps a little past itself.
const ROW = 30;
const SLOP = (TOUCH - ROW) / 2;

/**
 * Showing the file at `path` in the viewer, at `line`, when `when` holds and
 * the viewer opens a file of its kind.
 */
const useShowFile = (path: string, line: number | undefined, when: boolean): ShowFile | null => {
	const { t } = useI18n();
	const { agent } = useFeed();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const route = when ? fileRoute(agent, path, line) : null;
	return route
		? {
				label: t("Show {{name}}", { name: nameOf(path) }),
				onPress: () => navigation.navigate("File", route),
			}
		: null;
};

const ShowGlyph = ({ glyph: Shown = Eye }: { glyph?: Glyph }) => (
	<Shown size={13} strokeWidth={2} color={color.mutedForeground} />
);

const ToolRow = ({
	icon,
	tint,
	verb,
	target,
	meta,
	since,
	running = false,
	open,
	onToggle,
	show,
	children,
}: {
	icon: Glyph;
	/** The icon's color, when it isn't the muted one. */
	tint?: string;
	verb: string;
	target?: ReactNode;
	/** The figure at the end, such as "214 lines" or "Exit 0, 7.1 s". */
	meta?: ReactNode;
	/** When it started, for the clock a running step shows. */
	since?: Timestamp | null;
	running?: boolean;
	/** Whether the details show. Leave unset for a row with none. */
	open?: boolean;
	onToggle?: () => void;
	/**
	 * Shows the file in the viewer: a tap on a row with no details, and the
	 * eye at the end of one with them.
	 */
	show?: ShowFile | null;
	/** The details, shown while open. */
	children?: ReactNode;
}) => {
	const expandable = open !== undefined && onToggle !== undefined;
	const Chevron = open ? ChevronDown : ChevronRight;
	const line = (
		<>
			<View style={styles.icon}>
				{running ? <Spinner size={12} /> : <Icon glyph={icon} tint={tint} />}
			</View>
			<Text style={styles.verb}>{verb}</Text>
			{target}
			<Text style={styles.meta}>
				{running && since != null ? (
					<Elapsed since={since} format={formatTook} style={styles.clock} />
				) : (
					meta
				)}
			</Text>
			{expandable ? (
				<Chevron size={10} strokeWidth={2.4} color={color.mutedForeground} />
			) : show ? (
				<ShowGlyph glyph={show.glyph} />
			) : null}
		</>
	);
	const toggle = expandable ? (
		<Pressable
			accessibilityRole="button"
			accessibilityState={{ expanded: open }}
			onPress={onToggle}
			style={({ pressed }) => [
				styles.row,
				show ? styles.grow : null,
				pressed ? styles.pressed : null,
			]}
		>
			{line}
		</Pressable>
	) : null;
	return (
		<View style={styles.step}>
			{toggle && show ? (
				<View style={styles.split}>
					{toggle}
					<Pressable
						accessibilityRole="button"
						accessibilityLabel={show.label}
						onPress={show.onPress}
						hitSlop={SLOP}
						style={({ pressed }) => [styles.show, pressed ? styles.pressed : null]}
					>
						<ShowGlyph glyph={show.glyph} />
					</Pressable>
				</View>
			) : toggle ? (
				toggle
			) : show ? (
				<Pressable
					accessibilityRole="button"
					accessibilityHint={show.label}
					onPress={show.onPress}
					style={({ pressed }) => [styles.row, pressed ? styles.pressed : null]}
				>
					{line}
				</Pressable>
			) : (
				<View style={styles.row}>{line}</View>
			)}
			{expandable && open ? children : null}
		</View>
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
	const { agent } = useFeed();
	return images.length === 0 ? (
		children
	) : (
		<View style={styles.step}>
			{children}
			<PictureThumbs agent={agent} files={images} style={styles.indent} />
		</View>
	);
};

const TAIL = 12;

// A command in full, since a phone's row cuts it soon, then its output, with
// only the last lines until you ask for all of them.
const Output = ({
	id,
	command,
	text,
	truncated,
}: {
	id: string;
	command: string;
	text: string;
	truncated: boolean;
}) => {
	const { t } = useI18n();

	const [all, setAll] = useFold(`all:${id}`, id);
	const lines = useMemo(() => text.replace(/\n$/, "").split("\n"), [text]);
	const hidden = all ? 0 : Math.max(0, lines.length - TAIL);
	const shown = useMemo(() => lines.slice(hidden).join("\n"), [lines, hidden]);
	return (
		<Detail>
			<Text selectable style={[styles.code, styles.command]}>
				{command}
			</Text>
			{hidden > 0 ? (
				<Pressable
					accessibilityRole="button"
					onPress={() => setAll(true)}
					style={({ pressed }) => [styles.earlier, pressed ? styles.pressed : null]}
				>
					<ChevronUp size={10} strokeWidth={2.4} color={color.mutedForeground} />
					<Text style={styles.earlierText}>
						{t("Show {{count}} earlier lines", { count: hidden })}
					</Text>
				</Pressable>
			) : null}
			{text === "" ? (
				<Text style={styles.empty}>{t("No output")}</Text>
			) : (
				<ScrollView
					horizontal
					showsHorizontalScrollIndicator={false}
					contentContainerStyle={styles.output}
				>
					<AnsiText output={shown} />
				</ScrollView>
			)}
			{truncated ? (
				<Text style={styles.kept}>{t("MaxHQ kept only part of the output.")}</Text>
			) : null}
		</Detail>
	);
};

/** A shell command, with its output when opened. */
export const CommandRow = ({ item }: { item: ItemOf<"command"> }) => {
	const { t } = useI18n();

	const [open, setOpen] = useFold(`open:${item.id}`, item.id);
	const took = tookOf(item);
	const failed = item.exit_code !== null && item.exit_code !== 0;
	const exit =
		item.exit_code !== null ? (
			<Text style={failed ? styles.failed : undefined}>
				{took !== null
					? t("Exit {{code}}, {{duration}}", {
							code: formatNumber(item.exit_code),
							duration: formatTook(took),
						})
					: t("Exit {{code}}", { code: formatNumber(item.exit_code) })}
			</Text>
		) : null;
	return (
		<ToolRow
			icon={Terminal}
			verb={verbFor(item.status, TOOL_VERBS.command)}
			target={<Target>{item.command.split("\n")[0] ?? ""}</Target>}
			meta={
				endedEarly(item.status) ??
				exit ??
				(item.background
					? t("In the background")
					: item.status === "waiting"
						? t("Waiting for you")
						: null)
			}
			since={item.started_at}
			running={isOpen(item.status) && item.status !== "waiting"}
			open={open}
			onToggle={() => setOpen(!open)}
		>
			<Output
				id={item.id}
				command={item.command}
				text={item.output}
				truncated={item.output_truncated}
			/>
		</ToolRow>
	);
};

/**
 * A command you ran with `!`: how long it has run, that it waits for you,
 * or how it ended, with its output when opened once it ended. A program
 * that takes the whole screen, or reads keys one at a time, says to open
 * it on the Mac, since the phone has no keys to give it.
 */
export const UserRunRow = ({ item }: { item: ItemOf<"user_run"> }) => {
	const { t } = useI18n();
	const terminal = useView((s) => terminalById(s.view, item.terminal));
	const [open, setOpen] = useFold(`open:${item.id}`, item.id);
	const status = runStatus(terminal, item) ?? item.process;
	const waits = terminal !== undefined && waitsForYou(terminal);
	const handoff = handoffOf(status.state === "running", terminal);
	const ended = status.state === "exited" ? status : null;
	const failed = ended !== null && ended.stopped !== true && ended.code !== 0;
	return (
		<ToolRow
			icon={Terminal}
			verb={t("You ran")}
			target={<Target>{item.command.split("\n")[0] ?? ""}</Target>}
			meta={
				handoff !== null ? (
					handoffPhoneText(handoff)
				) : waits ? (
					t("Waiting for you")
				) : ended ? (
					<Text style={failed ? styles.failed : undefined}>
						{endedText(ended, item.duration_ms)}
					</Text>
				) : null
			}
			since={terminal?.started_at ?? item.started_at}
			running={status.state === "running" && !waits && handoff === null}
			open={ended ? open : undefined}
			onToggle={() => setOpen(!open)}
		>
			<Output
				id={item.id}
				command={item.command}
				text={item.output}
				truncated={item.output_truncated}
			/>
		</ToolRow>
	);
};

const EDIT_ICON: Record<FileEdit["op"], Glyph> = {
	add: FilePlus,
	modify: Pencil,
	delete: FileMinus,
	rename: FileSymlink,
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
	const [open, setOpen] = useKept(`open:${id}`, false);
	const changes = useMemo(() => [edit], [edit]);
	const show = useShowFile(
		edit.path,
		editLine(edit.patch),
		status === "completed" && edit.op !== "delete"
	);
	return (
		<ToolRow
			icon={EDIT_ICON[edit.op]}
			verb={verbFor(status, EDIT_VERBS[edit.op])}
			target={<Target path>{editPath(edit)}</Target>}
			meta={
				endedEarly(status) ?? (
					<Counts additions={edit.additions} deletions={edit.deletions} />
				)
			}
			since={since}
			running={status === "running" || status === "pending"}
			open={open}
			onToggle={() => setOpen(!open)}
			show={show}
		>
			<PatchView changes={changes} style={styles.patch} />
		</ToolRow>
	);
};

/** Edits to files, one row each. */
export const FileChangeRow = ({ item }: { item: ItemOf<"file_change"> }) => {
	return (
		<View style={styles.edits}>
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
		</View>
	);
};

/** A file the agent read. */
export const FileReadRow = ({ item }: { item: ItemOf<"file_read"> }) => {
	const show = useShowFile(item.path, readLine(item.lines), item.status === "completed");
	return (
		<WithPictures images={item.images}>
			<ToolRow
				icon={FileText}
				verb={verbFor(item.status, TOOL_VERBS.read)}
				target={<Target path>{item.path}</Target>}
				meta={endedEarly(item.status) ?? lineSpan(item.lines)}
				since={item.started_at}
				running={isOpen(item.status)}
				show={show}
			/>
		</WithPictures>
	);
};

/**
 * A file the agent opened in MaxHQ's viewer for you, named with its folder
 * and line. A tap shows it in the phone's viewer, at that line.
 */
export const OpenedFileRow = ({ item }: { item: ItemOf<"agent_opened_file"> }) => {
	const { t } = useI18n();
	const show = useShowFile(item.path, item.line ?? undefined, true);
	return (
		<ToolRow
			icon={Eye}
			show={show}
			verb={t("Opened")}
			target={
				<>
					<Target path most="50%">
						{nameOf(item.path)}
					</Target>
					<Text style={styles.verbless}>{t("in")}</Text>
					<Target path>{parentOf(item.path) ?? "/"}</Target>
				</>
			}
			meta={
				item.line != null ? t("Line {{number}}", { number: formatNumber(item.line) }) : null
			}
		/>
	);
};

/** A web page the agent showed you, such as a chart, which opens whole. */
export const VisualRow = ({ item }: { item: ItemOf<"agent_visual"> }) => {
	const { t } = useI18n();
	const { agent } = useFeed();
	const origin = useFeedOrigin(item);
	const visual = useMemo(() => visualOfItem(agent, item), [agent, item]);
	return (
		<VisualOriginContext value={origin}>
			<VisualOpenRow visual={visual} meta={item.replaces ? t("New version") : null} />
		</VisualOriginContext>
	);
};

/** A search through files. */
export const SearchRow = ({ item }: { item: ItemOf<"search"> }) => {
	const { t } = useI18n();
	return (
		<ToolRow
			icon={Search}
			verb={verbFor(item.status, TOOL_VERBS.search)}
			target={
				<>
					<Target>{item.query}</Target>
					{item.scope ? (
						<>
							<Text style={styles.verbless}>{t("in")}</Text>
							<Target path most="40%">
								{item.scope}
							</Target>
						</>
					) : null}
				</>
			}
			meta={
				endedEarly(item.status) ??
				(item.matches === null
					? null
					: item.matches === 0
						? t("No matches")
						: t("{{count}} matches", { count: item.matches }))
			}
			since={item.started_at}
			running={isOpen(item.status)}
		/>
	);
};

/** A web search or a page fetched. */
export const WebRow = ({ item }: { item: ItemOf<"web"> }) => {
	return (
		<ToolRow
			icon={Globe}
			verb={verbFor(item.status, WEB_VERBS[item.action])}
			target={<Target>{item.target}</Target>}
			meta={endedEarly(item.status)}
			since={item.started_at}
			running={isOpen(item.status)}
		/>
	);
};

const Section = ({
	label,
	text,
	ansi = false,
}: {
	label: string;
	text: string;
	ansi?: boolean;
}) => {
	return (
		<View style={styles.section}>
			<Text style={styles.sectionLabel}>{label}</Text>
			<ScrollView nestedScrollEnabled style={styles.sectionScroll}>
				{ansi ? (
					<AnsiText output={text} />
				) : (
					<Text selectable style={styles.code}>
						{text}
					</Text>
				)}
			</ScrollView>
		</View>
	);
};

const CallRow = ({
	item,
	icon = Wrench,
	verb,
	target,
	note,
	show,
}: {
	item: ItemOf<"tool_call">;
	icon?: Glyph;
	verb?: string;
	target?: ReactNode;
	note?: string | null;
	show?: ShowFile | null;
}) => {
	const { t } = useI18n();

	const [open, setOpen] = useFold(`open:${item.id}`, item.id);
	const name = item.server ? `${item.server}:${item.name}` : item.name;
	const hasDetail = Boolean(item.input || item.output);
	const took = tookOf(item);
	return (
		<WithPictures images={item.images}>
			<ToolRow
				icon={icon}
				verb={verb ?? verbFor(item.status, TOOL_VERBS.call)}
				target={
					target ?? (
						<>
							<Target most="45%">{name}</Target>
							{item.title ? (
								<Text numberOfLines={1} style={styles.title}>
									{item.title}
								</Text>
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
				show={show}
			>
				<Detail>
					{item.input ? <Section label={t("Input")} text={item.input} /> : null}
					{item.output ? (
						<Section
							ansi
							label={
								lineCount(item.output) > 1
									? t("Output, {{count}} lines", {
											count: lineCount(item.output),
										})
									: t("Output")
							}
							text={item.output}
						/>
					) : null}
				</Detail>
			</ToolRow>
		</WithPictures>
	);
};

// A call to one of MaxHQ's agent tools, in words: "Started Fix the login bug
// on devbox" or "Waiting on 2 agents".
const AgentToolCallRow = ({ item }: { item: ItemOf<"tool_call"> }) => {
	const view = useView((s) => s.view);
	const line = agentToolLine(item, viewNames(view));
	return line ? (
		<CallRow
			item={item}
			icon={Bot}
			verb={verbFor(item.status, line)}
			target={<Target>{line.target}</Target>}
			note={line.note}
		/>
	) : (
		<CallRow item={item} />
	);
};

const ARTIFACT_GLYPH: Record<ArtifactTool, Glyph> = {
	page: AppWindow,
	data: Database,
	comments: MessageSquare,
	check: BadgeCheck,
};

// Opens a claude.ai page in the browser, checking the link once more first.
const openPage = (url: string) => {
	const href = claudeLink(url);
	if (href) Linking.openURL(href).catch(() => {});
};

// A call to Claude Code's tools for claude.ai pages, in words: "Published
// Weekly report", with a tap at its end that opens the page in the browser.
const ArtifactCallRow = ({ item, line }: { item: ItemOf<"tool_call">; line: ArtifactLine }) => {
	const { t } = useI18n();
	const { url } = line;
	const show = url
		? {
				label: t("Open {{name}} in the browser", { name: line.target || url }),
				onPress: () => openPage(url),
				glyph: ExternalLink,
			}
		: null;
	return (
		<CallRow
			item={item}
			icon={ARTIFACT_GLYPH[line.tool]}
			verb={verbFor(item.status, line.verbs)}
			target={line.target ? <Target>{line.target}</Target> : false}
			note={line.note}
			show={show}
		/>
	);
};

/** Any other tool, such as one from an MCP server. */
export const ToolCallRow = ({ item }: { item: ItemOf<"tool_call"> }) => {
	const artifact = artifactLine(item);
	if (artifact) return <ArtifactCallRow item={item} line={artifact} />;
	return agentToolOf(item.server, item.name) ? (
		<AgentToolCallRow item={item} />
	) : (
		<CallRow item={item} />
	);
};

const styles = StyleSheet.create({
	step: { gap: 4 },
	edits: { gap: 1 },
	row: {
		minHeight: 30,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingLeft: 6,
		paddingRight: 8,
		borderRadius: radius.md,
	},
	pressed: { backgroundColor: color.accent },
	// An edit's row and the eye that shows its file, side by side.
	split: { flexDirection: "row", alignItems: "center", gap: 2 },
	grow: { flex: 1, minWidth: 0 },
	show: {
		width: ROW,
		height: ROW,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: radius.md,
	},
	icon: { width: 14, height: 14, alignItems: "center", justifyContent: "center" },
	verb: { flexShrink: 0, color: color.secondaryForeground, fontSize: 12.5 },
	verbless: { flexShrink: 0, color: color.mutedForeground, fontSize: 12.5 },
	title: { flexShrink: 1, minWidth: 0, color: color.mutedForeground, fontSize: 12.5 },
	target: {
		flexShrink: 1,
		minWidth: 0,
		color: color.terminal,
		fontFamily: font.mono,
		fontSize: size.tiny,
	},
	meta: {
		flexShrink: 0,
		marginLeft: "auto",
		paddingLeft: 8,
		color: color.mutedForeground,
		fontSize: size.tiny,
		fontVariant: ["tabular-nums"],
	},
	clock: { color: color.busyText },
	failed: { color: color.errorText },
	indent: { marginLeft: 28 },
	detail: {
		marginLeft: 28,
		overflow: "hidden",
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	patch: { marginLeft: 28, borderRadius: radius.md },
	code: {
		color: color.secondaryForeground,
		fontFamily: font.mono,
		fontSize: size.tiny,
		lineHeight: 19,
	},
	command: {
		paddingHorizontal: 10,
		paddingTop: 6,
		paddingBottom: 7,
		color: color.terminal,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.secondary,
	},
	earlier: {
		height: 24,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 10,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.secondary,
	},
	earlierText: { color: color.mutedForeground, fontSize: size.tiny },
	empty: {
		paddingHorizontal: 10,
		paddingVertical: 6,
		color: color.mutedForeground,
		fontSize: size.tiny,
	},
	output: { paddingHorizontal: 10, paddingTop: 6, paddingBottom: 7 },
	kept: {
		paddingHorizontal: 10,
		paddingVertical: 4,
		color: color.subtleForeground,
		fontSize: size.pill,
		borderTopWidth: StyleSheet.hairlineWidth,
		borderTopColor: color.secondary,
	},
	section: {
		paddingHorizontal: 10,
		paddingTop: 6,
		paddingBottom: 6,
		gap: 2,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.secondary,
	},
	sectionLabel: { color: color.subtleForeground, fontSize: size.pill },
	sectionScroll: { maxHeight: 320 },
});
