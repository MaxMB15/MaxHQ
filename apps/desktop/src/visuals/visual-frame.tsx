// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual drawn in its frame: a sandboxed page on an opaque origin with no
// network, that gets the page in a message and writes it. The frame grows
// with its page, up to a limit, and stops following a page that grows
// because it grew. It draws in the app's theme and draws again in a new one.
// Above it, bars say what went wrong: the page's errors, which Tell the
// agent quotes back, the hosts the network rule blocked, which Load this
// time lets through until the frame goes, and the files it asked for that
// aren't there. In a tab, the frame fills its pane instead and the page
// scrolls inside it, at a zoom that lays the page out again.
//
// A frame's scripts run on the window's thread, so a page that never ends
// stops the window. One that was running when the window went away waits
// for Run anyway the next time.

import { t, useI18n, formatList, formatNumber } from "@maxhq/shared/i18n";

import { type CSSProperties, useEffect, useMemo, useReducer, useRef, useState } from "react";

import { CodeBlock } from "@/agent-view/code-block";
import { useSeen } from "@/agent-view/picture-viewer";
import { useTheme } from "@/app/theme";
import { hangGuard } from "@/lib/hang-guard";
import { cn } from "@/lib/utils";
import { framePageUrl } from "@/viewer/load";
import { FRAME_SANDBOX } from "@/viewer/messages";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import type { Theme } from "@maxhq/shared/theme/theme";
import {
	HEIGHT_START,
	type HeightTrack,
	nextHeight,
	readFrameReport,
	type ShowVisual,
	THEME_VARIABLES,
	visualFrameQuery,
} from "@maxhq/shared/visuals/frame";
import {
	addReport,
	loadableHosts,
	NO_REPORTS,
	type VisualError,
	type VisualReports,
	withHosts,
} from "@maxhq/shared/visuals/reports";
import { MAX_VISUAL_BYTES, visualBytes, visualKey } from "@maxhq/shared/visuals/visual";

import { FIRST_HEIGHT_PX, knownHeight, noteHeight } from "./heights";

const guard = hangGuard("visual");

export type { VisualError };

const MONO_FALLBACK = '"JetBrains Mono", ui-monospace, "SF Mono", Menlo, monospace';

// The theme's values as the app has them now. A font the app sets only
// through its styles falls back to what the page shows.
const themeVariables = (): Record<string, string> => {
	const styles = getComputedStyle(document.documentElement);
	const values: Record<string, string> = {};
	for (const name of THEME_VARIABLES) {
		const value = styles.getPropertyValue(name).trim();
		if (value) values[name] = value;
	}
	values["--font-sans"] ||= getComputedStyle(document.body).fontFamily;
	values["--font-mono"] ||= MONO_FALLBACK;
	return values;
};

// A page can say it was clicked when it wasn't, so the app opens a link only
// while its frame has the focus a click gives it, and one at a time.
const LINK_GAP_MS = 1000;
let lastLink = 0;

const openLink = (url: string) => {
	const now = Date.now();
	if (now - lastLink < LINK_GAP_MS) return;
	lastLink = now;
	window.open(url, "_blank", "noreferrer");
};

const BAR = "flex items-start gap-2 border-b px-3 py-1.5 text-[11.5px] leading-[1.45]";

const BAR_BUTTON =
	"h-[20px] shrink-0 cursor-pointer rounded-[5px] border border-current/30 px-1.5 text-[11px] outline-none hover:bg-current/10 focus-visible:ring-2 focus-visible:ring-ring";

/** The first error, its line when known, and how many more there are. */
const errorLine = ({ message, line }: VisualError, count: number): string => {
	const first = line === null ? message : t("{{message}} (line {{line}})", { message, line });
	return count > 1
		? t("{{error}}, and {{countText}} more errors", {
				error: first,
				count: count - 1,
				countText: formatNumber(count - 1),
			})
		: first;
};

/** What went wrong in a visual's page, each kind on a bar of its own. */
const Bars = ({
	reports,
	hosts,
	onTell,
	onLoad,
}: {
	reports: VisualReports;
	hosts: readonly string[];
	onTell: ((errors: readonly VisualError[], count: number) => void) | undefined;
	onLoad: (hosts: readonly string[]) => void;
}) => {
	const { t } = useI18n();
	const [first] = reports.errors;
	const blocked = [...reports.blocked.values()];
	const loadable = loadableHosts(reports.blocked, hosts);
	return (
		<>
			{first ? (
				<div
					role="alert"
					className={cn(
						BAR,
						"border-status-error/30 bg-status-error/10 text-status-error-text"
					)}
				>
					<p className="min-w-0 flex-1 break-words">
						{errorLine(first, reports.errorCount)}
					</p>
					{onTell ? (
						<button
							type="button"
							className={BAR_BUTTON}
							onClick={() => onTell(reports.errors, reports.errorCount)}
						>
							{t("Tell the agent")}
						</button>
					) : null}
				</div>
			) : null}
			{blocked.length > 0 ? (
				<div
					className={cn(
						BAR,
						"border-status-busy/30 bg-status-busy/10 text-status-busy-text"
					)}
				>
					<p className="min-w-0 flex-1 break-words">
						{t("Blocked loading from {{hosts}}. Visuals don't reach the network.", {
							hosts: formatList(blocked.map((b) => b.host)),
						})}
						{loadable.length > 0
							? ` ${t("Loading runs code from {{hosts}} in this visual, and tells them your address and that you opened it.", { hosts: formatList(loadable) })}`
							: ""}
					</p>
					{loadable.length > 0 ? (
						<button
							type="button"
							className={BAR_BUTTON}
							onClick={() => onLoad(loadable)}
						>
							{t("Load this time")}
						</button>
					) : null}
				</div>
			) : null}
			{reports.missing.length > 0 ? (
				<div className={cn(BAR, "border-border bg-subpanel text-muted-foreground")}>
					<p className="min-w-0 flex-1 break-words">
						{t(
							"Not found: {{paths}}. A visual has only its own page and MaxHQ's libraries.",
							{
								paths: formatList(reports.missing),
							}
						)}
					</p>
				</div>
			) : null}
		</>
	);
};

/**
 * How a frame that fills its box draws at `zoom`: laid out in the box's size
 * divided by the zoom, then scaled back up to fill it, as a browser's zoom
 * lays a page out again rather than scaling a picture of it.
 */
export const zoomedFrame = (zoom: number): CSSProperties => ({
	width: `${100 / zoom}%`,
	height: `${100 / zoom}%`,
	transform: zoom === 1 ? undefined : `scale(${zoom})`,
	transformOrigin: "0 0",
});

/** The frame page's address, with the theme and the hosts let through. */
const frameSrc = (theme: Theme, hosts: readonly string[]) =>
	framePageUrl("visual.html") + visualFrameQuery(theme, hosts);

/** One run of a page in a frame. A new theme or new hosts make a new run. */
const FrameRun = ({
	html,
	mark,
	title,
	theme,
	hosts,
	fill,
	onTell,
	onLoad,
}: {
	html: string;
	mark: string;
	title: string;
	theme: Theme;
	hosts: readonly string[];
	fill: { readonly zoom: number } | undefined;
	onTell: ((errors: readonly VisualError[], count: number) => void) | undefined;
	onLoad: (hosts: readonly string[]) => void;
}) => {
	const { t } = useI18n();
	const frame = useRef<HTMLIFrameElement>(null);
	const [track, setTrack] = useState<HeightTrack>(() => ({
		...HEIGHT_START,
		height: knownHeight(mark) ?? FIRST_HEIGHT_PX,
	}));
	const [reports, report] = useReducer(addReport, NO_REPORTS);
	const reported = useRef(false);

	useEffect(() => {
		if (reported.current) noteHeight(mark, track.height);
	}, [mark, track.height]);

	// The listener is in place before the frame loads, so it hears the frame's
	// first word.
	useEffect(() => {
		let shown = false;
		let running = false;
		const end = () => {
			if (!running) return;
			running = false;
			guard.finish(mark);
		};
		const onMessage = (event: MessageEvent) => {
			const into = frame.current?.contentWindow;
			if (!into || event.source !== into) return;
			const said = readFrameReport(event.data);
			switch (said?.type) {
				case undefined:
					return;
				case "maxhq-visual:ready": {
					if (shown) return;
					shown = true;
					running = true;
					guard.start(mark);
					const show: ShowVisual = {
						type: "maxhq-visual:show",
						html,
						theme,
						variables: themeVariables(),
					};
					// The frame's origin is opaque, so there's no name to send to.
					into.postMessage(show, "*");
					return;
				}
				case "maxhq-visual:height":
					reported.current = true;
					setTrack((track) => nextHeight(track, said.height));
					return;
				case "maxhq-visual:settled":
					end();
					return;
				case "maxhq-visual:link":
					if (document.activeElement === frame.current) openLink(said.url);
					return;
				case "maxhq-visual:error":
					report({ type: "error", error: { message: said.message, line: said.line } });
					return;
				case "maxhq-visual:blocked":
					report({ type: "blocked", url: said.url, what: said.what });
					return;
				case "maxhq-visual:missing":
					report({ type: "missing", path: said.path });
			}
		};
		window.addEventListener("message", onMessage);
		// The page loads only once this listens, so its first word isn't lost.
		frame.current?.setAttribute("src", frameSrc(theme, hosts));
		return () => {
			window.removeEventListener("message", onMessage);
			end();
		};
	}, [html, mark, theme, hosts]);

	return (
		<>
			<Bars reports={reports} hosts={hosts} onTell={onTell} onLoad={onLoad} />
			{fill ? (
				<div className="relative min-h-0 flex-1 overflow-hidden">
					<iframe
						ref={frame}
						sandbox={FRAME_SANDBOX}
						title={title || t("Visual")}
						style={{ ...zoomedFrame(fill.zoom), colorScheme: theme }}
						className="absolute top-0 left-0 block border-0 bg-transparent"
					/>
				</div>
			) : (
				<iframe
					ref={frame}
					sandbox={FRAME_SANDBOX}
					title={title || t("Visual")}
					// The same scheme as the page's, so the frame stays clear
					// rather than painting its own ground first.
					style={{ height: track.height, colorScheme: theme }}
					className="block w-full border-0 bg-transparent"
				/>
			)}
		</>
	);
};

const HELD_BUTTON =
	"h-[24px] cursor-pointer rounded-[5px] border border-border px-2 text-[11.5px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring";

/** A visual's page in its frame, with what went wrong above it. */
export const VisualFrame = ({
	html,
	title,
	runs = 0,
	fill,
	onTell,
	className,
}: {
	html: string;
	title: string;
	/** Counts Reload, each of which runs the page again in a new frame. */
	runs?: number;
	/**
	 * Fills the box, which lays its bars and frame out in a column, with
	 * the page scrolling inside at this zoom, instead of growing with it.
	 */
	fill?: { readonly zoom: number };
	/** Quotes the page's errors to the agent that wrote it, when there is one. */
	onTell?: (errors: readonly VisualError[], count: number) => void;
	className?: string;
}) => {
	const { t } = useI18n();
	const mark = useMemo(() => visualKey(html), [html]);
	const bytes = useMemo(() => visualBytes(html), [html]);
	const theme = useTheme();
	const [ref, seen] = useSeen<HTMLDivElement>();
	const [hosts, setHosts] = useState<readonly string[]>([]);
	const [allowed, setAllowed] = useState(false);
	const [source, setSource] = useState(false);
	const held = useMemo(() => !allowed && guard.held(mark), [allowed, mark]);

	if (bytes > MAX_VISUAL_BYTES)
		return (
			<div className={cn("flex flex-col gap-1 p-2", className)}>
				<p className="text-[11.5px] text-muted-foreground">
					{t(
						"This visual is {{size}}, more than the {{most}} MaxHQ draws, so it shows as its source.",
						{
							size: formatBytes(bytes),
							most: formatBytes(MAX_VISUAL_BYTES),
						}
					)}
				</p>
				<CodeBlock code={html} lang="html" />
			</div>
		);
	if (held)
		return (
			<div className={cn("flex flex-col gap-2 p-3", className)}>
				<p className="text-[12px] text-muted-foreground">
					{t(
						"MaxHQ stopped responding while this visual ran last time, so it waits until you run it. If it stops MaxHQ again, quit and reopen it, and the visual waits again."
					)}
				</p>
				<div className="flex gap-2">
					<button
						type="button"
						className={HELD_BUTTON}
						onClick={() => {
							guard.allow(mark);
							setAllowed(true);
						}}
					>
						{t("Run anyway")}
					</button>
					<button
						type="button"
						aria-pressed={source}
						className={HELD_BUTTON}
						onClick={() => setSource((shown) => !shown)}
					>
						{source ? t("Hide source") : t("View source")}
					</button>
				</div>
				{source ? <CodeBlock code={html} lang="html" /> : null}
			</div>
		);
	return (
		<div ref={ref} className={className}>
			{seen ? (
				<FrameRun
					key={`${theme}\n${hosts.join(",")}\n${runs}`}
					html={html}
					mark={mark}
					title={title}
					theme={theme}
					hosts={hosts}
					fill={fill}
					onTell={onTell}
					onLoad={(more) => setHosts((hosts) => withHosts(hosts, more))}
				/>
			) : (
				<div style={{ height: knownHeight(mark) ?? FIRST_HEIGHT_PX }} />
			)}
		</div>
	);
};
