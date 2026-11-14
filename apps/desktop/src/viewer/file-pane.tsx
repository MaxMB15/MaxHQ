// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file pane: a file from an agent's device, shown read-only. Its bytes come
// through the media cache, the way the timeline's pictures do. Markdown
// draws with the app's own Markdown, and code and plain text draw as
// read-only text with the chat's colours. An HTML page draws as a visual,
// with its source a click away. Every other kind goes to the
// viewer frame, a sandboxed page on an opaque origin with no network, which gets
// the bytes in a message and draws them. The header names the file and
// opens it in an editor.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useEffect, useMemo, useRef, useState } from "react";

import { Markdown } from "@/agent-view/markdown";
import { OpenInButton } from "@/agent-view/open-in";
import { useTimeline } from "@/agent-view/use-timeline";
import { useServices, useView } from "@/app/services";
import { useTheme } from "@/app/theme";
import { Empty } from "@/main-area/empty";
import { SECONDARY } from "@/settings/controls";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import { loadMedia, useMedia } from "@maxhq/shared/agent-view/media";
import {
	extensionOf,
	fileName,
	showAnywayBlocked,
	viewerKind,
	viewerLimit,
} from "@maxhq/shared/agent-view/viewer-files";
import type { AgentId } from "@maxhq/shared/bindings";
import type { Theme } from "@maxhq/shared/theme/theme";
import { isRemoteDevice } from "@/ports";
import { VisualFrame } from "@/visuals/visual-frame";

import { CodeFile, tooBigNote } from "./code-file";
import { readBytes, viewerFrameUrl } from "./load";
import { FRAME_SANDBOX, type OpenFile, isFromFrame } from "./messages";

const noSubAgents = () => {};

/** A Markdown file, drawn as the timeline draws a reply, HTML included. */
const MarkdownFile = ({ agent, url, size }: { agent: AgentId; url: string; size: number }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [text, setText] = useState<string | null>(null);
	const [failure, setFailure] = useState<string | null>(null);
	useEffect(() => {
		let current = true;
		readBytes(url, size).then(
			(bytes) => current && setText(new TextDecoder().decode(bytes)),
			(error: unknown) => current && setFailure(String(error))
		);
		return () => {
			current = false;
		};
	}, [url, size]);
	// Pictures the file names come from the agent's device, which they find
	// through the row the timeline would give them.
	const timeline = useTimeline(agent);
	const found = useView((s) => s.view.tables.agent.get(agent));
	const device = useView((s) =>
		found ? s.view.tables.workspace.get(found.workspace)?.device : undefined
	);
	const remote = useView((s) => (device ? isRemoteDevice(s.view, device) : false));
	const row = useMemo<RowContext | null>(
		() =>
			found
				? {
						agent,
						vendor: found.vendor,
						timeline: timeline.store,
						readOnly: true,
						remote,
						answer: (attention, answer, refs) => source.answer(attention, answer, refs),
						subAgentsShown: false,
						toggleSubAgents: noSubAgents,
					}
				: null,
		[agent, found, timeline.store, remote, source]
	);
	if (failure) return <Empty>{failure}</Empty>;
	if (text === null || !row) return <Empty>{t("Reading…")}</Empty>;
	return (
		<div className="h-full overflow-y-auto">
			<RowProvider value={row}>
				<Markdown text={text} className="mx-auto max-w-[780px] px-8 py-6" />
			</RowProvider>
		</div>
	);
};

const SHOWN_AS = [
	{
		rendered: true,
		get label() {
			return t("Rendered");
		},
	},
	{
		rendered: false,
		get label() {
			return t("Source");
		},
	},
] as const;

/** An HTML page, drawn as a visual, or its source as code. */
const HtmlFile = ({
	agent,
	path,
	url,
	size,
}: {
	agent: AgentId;
	path: string;
	url: string;
	size: number;
}) => {
	const { t } = useI18n();
	const [rendered, setRendered] = useState(true);
	const [text, setText] = useState<string | null>(null);
	const [failure, setFailure] = useState<string | null>(null);
	useEffect(() => {
		if (!rendered || text !== null) return;
		let current = true;
		readBytes(url, size).then(
			(bytes) => current && setText(new TextDecoder().decode(bytes)),
			(error: unknown) => current && setFailure(String(error))
		);
		return () => {
			current = false;
		};
	}, [rendered, text, url, size]);
	return (
		<div className="flex h-full flex-col">
			<div className="flex h-8 shrink-0 items-center gap-1 border-b border-border px-2">
				{SHOWN_AS.map((as) => (
					<button
						key={as.label}
						type="button"
						aria-pressed={rendered === as.rendered}
						onClick={() => setRendered(as.rendered)}
						className="h-[22px] rounded-[6px] px-2 text-[12px] text-secondary-foreground hover:bg-secondary aria-pressed:bg-selected aria-pressed:text-foreground"
					>
						{as.label}
					</button>
				))}
			</div>
			<div className="min-h-0 flex-1">
				{!rendered ? (
					<CodeFile agent={agent} path={path} url={url} size={size} />
				) : failure ? (
					<Empty>{failure}</Empty>
				) : text === null ? (
					<Empty>{t("Reading…")}</Empty>
				) : (
					<div className="h-full overflow-y-auto">
						<VisualFrame html={text} title={fileName(path)} className="px-4 py-3" />
					</div>
				)}
			</div>
		</div>
	);
};

type Shown =
	{ readonly state: "opening" | "shown" } | { readonly state: "failed"; readonly why: string };

/**
 * Any other file, drawn by the viewer frame from bytes this pane hands it,
 * in the theme it opened in.
 */
const FramedFile = ({
	url,
	size,
	path,
	theme,
}: {
	url: string;
	size: number;
	path: string;
	theme: Theme;
}) => {
	const { t } = useI18n();

	const frame = useRef<HTMLIFrameElement>(null);
	const [shown, setShown] = useState<Shown>({ state: "opening" });
	const src = useMemo(() => viewerFrameUrl(), []);
	useEffect(() => {
		const fail = (error: unknown) =>
			setShown({
				state: "failed",
				why: error instanceof Error ? error.message : String(error),
			});
		const onMessage = (event: MessageEvent) => {
			const into = frame.current?.contentWindow;
			if (!into || event.source !== into || !isFromFrame(event.data)) return;
			const said = event.data;
			if (said.type === "maxhq-viewer:shown") setShown({ state: "shown" });
			else if (said.type === "maxhq-viewer:failed") fail(said.message);
			else {
				// The frame is ready, again if it reloaded, so it gets its own copy.
				readBytes(url, size).then((bytes) => {
					const open: OpenFile = {
						type: "maxhq-viewer:open",
						name: fileName(path),
						extension: extensionOf(path),
						bytes,
						theme,
					};
					// The frame's origin is opaque, so there's no name to send to.
					into.postMessage(open, "*", [bytes]);
				}, fail);
			}
		};
		window.addEventListener("message", onMessage);
		return () => window.removeEventListener("message", onMessage);
	}, [url, size, path, theme]);
	return (
		<div className="relative h-full">
			<iframe
				ref={frame}
				src={src}
				sandbox={FRAME_SANDBOX}
				title={fileName(path)}
				className="absolute inset-0 size-full border-0 bg-background"
			/>
			{shown.state !== "shown" ? (
				<div className="absolute inset-0 bg-background">
					<Empty>
						{shown.state === "failed"
							? t("Couldn't show {{value1}}. {{why}}", {
									value1: fileName(path),
									why: shown.why,
								})
							: t("Opening {{value1}}…", { value1: fileName(path) })}
					</Empty>
				</div>
			) : null}
		</div>
	);
};

/** What the pane shows below its header, as the file's fetch stands. */
const Body = ({ agent, path }: { agent: AgentId; path: string }) => {
	const { t } = useI18n();

	const { source } = useServices();
	// Show anyway holds for this pane only. Nothing remembers it.
	const [anyway, setAnyway] = useState(false);
	const media = useMedia(agent, path, anyway);
	const limit = viewerLimit(path);
	useEffect(() => {
		void loadMedia(source, agent, path, limit, anyway);
	}, [source, agent, path, limit, anyway]);
	const blocked = useView((s) => showAnywayBlocked(s.view, agent));
	const theme = useTheme();
	const name = fileName(path);
	switch (media?.state) {
		case undefined:
		case "loading":
			return (
				<Empty>
					{media && media.total > 0
						? t("Reading {{name}}, {{value2}} of {{value3}}…", {
								name: name,
								value2: formatBytes(media.sent),
								value3: formatBytes(media.total),
							})
						: t("Reading {{name}}…", { name: name })}
				</Empty>
			);
		case "large":
			return (
				<Empty>
					{viewerKind(path) === "code"
						? tooBigNote(path, formatBytes(media.size))
						: t(
								"{{name}} is {{value2}}, more than MaxHQ shows. Open it in an editor instead.",
								{ name: name, value2: formatBytes(media.size) }
							)}
				</Empty>
			);
		case "failed": {
			const outside = media.refused === "outside";
			// An older maxd says a bad path for one outside too, so the note
			// to update it goes with any refused path.
			const note = media.refused && !anyway ? blocked : null;
			const said = outside
				? t("{{name}} is outside the agent's folders.", { name: name })
				: t("Couldn't read {{name}}. {{reason}}", { name: name, reason: media.reason });
			return (
				<div className="grid h-full place-items-center px-8">
					<div className="flex max-w-[48ch] flex-col items-center gap-3 text-center">
						<p className="text-[13px] text-muted-foreground">
							{note ? `${said} ${note}` : said}
						</p>
						<div className="flex gap-2">
							{outside && !anyway && !blocked ? (
								<button
									type="button"
									className={SECONDARY}
									onClick={() => setAnyway(true)}
								>
									{t("Show anyway")}
								</button>
							) : null}
							<button
								type="button"
								className={SECONDARY}
								onClick={() => void loadMedia(source, agent, path, limit, anyway)}
							>
								{t("Try again")}
							</button>
						</div>
					</div>
				</div>
			);
		}
		case "ready":
			switch (viewerKind(path)) {
				case "markdown":
					return (
						<MarkdownFile
							key={media.url}
							agent={agent}
							url={media.url}
							size={media.size}
						/>
					);
				case "code":
					if (/^html?$/i.test(extensionOf(path)))
						return (
							<HtmlFile
								key={media.url}
								agent={agent}
								path={path}
								url={media.url}
								size={media.size}
							/>
						);
					return (
						<CodeFile
							key={media.url}
							agent={agent}
							path={path}
							url={media.url}
							size={media.size}
						/>
					);
				default:
					// A new theme opens the file again in a new frame.
					return (
						<FramedFile
							key={`${media.url}\n${theme}`}
							url={media.url}
							size={media.size}
							path={path}
							theme={theme}
						/>
					);
			}
	}
};

/** A file pane's contents: a header naming the file, then the file. */
export const FilePaneFor = ({ agent, path }: { agent: AgentId; path: string }) => {
	useI18n();

	const workspace = useView((s) => s.view.tables.agent.get(agent)?.workspace);
	const [failure, setFailure] = useState<string | null>(null);
	return (
		<section aria-label={fileName(path)} className="flex h-full min-h-0 flex-col bg-background">
			<header className="flex h-10 shrink-0 items-center gap-2 border-b border-border px-3">
				<span className="truncate text-[12.5px] font-[550] text-foreground">
					{fileName(path)}
				</span>
				<span className="min-w-0 flex-1 truncate text-[11.5px] text-muted-foreground">
					{path}
				</span>
				{workspace ? (
					<OpenInButton workspace={workspace} at={{ path }} onFail={setFailure} />
				) : null}
			</header>
			{failure ? (
				<p
					role="alert"
					className="border-b border-border px-3 py-1.5 text-[11.5px] text-status-error-text"
				>
					{failure}
				</p>
			) : null}
			<div className="min-h-0 flex-1">
				<Body key={`${agent}\n${path}`} agent={agent} path={path} />
			</div>
		</section>
	);
};
