// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Fetching a remote agent's files to this Mac. A file row or an attachment
// of an agent on another device gets a small button that downloads it into
// ~/Downloads/MaxHQ. The button shows how far it has got and cancels it,
// and once the file is here it shows it in Finder. The downloads live
// outside the rows, so one keeps going while its row scrolls away.

import { Check, Download, FolderOpen, Pause, RotateCw } from "lucide-react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { useServices } from "@/app/services";
import { useComposerNative } from "@/composer/native";
import type { DataSource, DownloadResult } from "@maxhq/shared/data/source";
import type { AgentId, TransferId } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { errorText } from "@maxhq/shared/timeline/store";

import { useRow } from "@maxhq/shared/agent-view/context";
import { baseName } from "@maxhq/shared/agent-view/format";
import { Spinner } from "./icons";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** Where one download stands. */
export type DownloadState =
	| {
			readonly state: "downloading";
			readonly sent: number;
			readonly total: number;
			/** Why it stopped for now: the link is down, or there's no room yet. */
			readonly held: "paused" | "waiting" | null;
	  }
	| {
			readonly state: "done";
			/** Where it landed on this Mac. */
			readonly path: string;
	  }
	| { readonly state: "failed"; readonly reason: string };

const store = createStore<{ downloads: ReadonlyMap<string, DownloadState> }>(() => ({
	downloads: new Map(),
}));

const keyFor = (agent: AgentId, path: string) => `${agent}\n${path}`;

const put = (key: string, state: DownloadState | null) =>
	store.setState(({ downloads }) => {
		const next = new Map(downloads);
		if (state) next.set(key, state);
		else next.delete(key);
		return { downloads: next };
	});

// A download on its way. Cancelling marks it, so the progress and the
// result that still come in change nothing.
type Job = { transfer: TransferId | null; cancelled: boolean };

const jobs = new Map<string, Job>();

/** Downloads a file an agent can see, unless it's already on its way. */
export const startDownload = async (
	source: DataSource,
	agent: AgentId,
	path: string
): Promise<void> => {
	const key = keyFor(agent, path);
	if (jobs.has(key)) return;
	const job: Job = { transfer: null, cancelled: false };
	jobs.set(key, job);
	put(key, { state: "downloading", sent: 0, total: 0, held: null });
	let result: DownloadResult;
	try {
		result = await source.download(agent, path, (progress) => {
			const first = job.transfer === null;
			job.transfer = progress.transfer;
			// Cancelled before the transfer had a name, so it stops now.
			if (job.cancelled) {
				if (first) void source.cancelTransfer(progress.transfer);
				return;
			}
			put(key, {
				state: "downloading",
				sent: progress.sent,
				total: progress.total,
				held:
					progress.state === "paused" || progress.state === "waiting"
						? progress.state
						: null,
			});
		});
	} catch (error) {
		result = {
			status: "error",
			error: {
				kind: "failed",
				message: error instanceof Error ? error.message : String(error),
			},
		};
	}
	if (job.cancelled) return;
	jobs.delete(key);
	put(
		key,
		result.status === "ok"
			? { state: "done", path: result.data.path }
			: { state: "failed", reason: errorText(result.error) }
	);
};

/** Stops a download and forgets it, so its button offers it again. */
export const cancelDownload = (source: DataSource, agent: AgentId, path: string): void => {
	const key = keyFor(agent, path);
	const job = jobs.get(key);
	if (job) {
		job.cancelled = true;
		jobs.delete(key);
		if (job.transfer) void source.cancelTransfer(job.transfer);
	}
	put(key, null);
};

/** Where a download of this file stands, or null when there's none. */
export const useDownload = (agent: AgentId, path: string): DownloadState | null =>
	useStore(store, (s) => s.downloads.get(keyFor(agent, path)) ?? null);

/** Forgets every download, for tests. */
export const clearDownloads = (): void => {
	for (const job of jobs.values()) job.cancelled = true;
	jobs.clear();
	store.setState({ downloads: new Map() });
};

const HELD_TITLE = {
	get active() {
		return translate("Downloading. Click to cancel.");
	},
	get paused() {
		return translate("Paused, reconnecting");
	},
	get waiting() {
		return translate("Waiting for room on this Mac");
	},
} as const;

const percent = (sent: number, total: number) =>
	total > 0 ? Math.min(100, Math.floor((sent / total) * 100)) : null;

/**
 * The download button for a file a remote agent can see, as a path relative
 * to its folder or absolute inside it. Renders nothing for an agent on
 * this Mac, whose files are already here.
 */
export const DownloadButton = ({
	path,
	name = baseName(path),
	className,
}: {
	path: string;
	/** The name in the labels. The path's last part when unset. */
	name?: string;
	className?: string;
}) => {
	useI18n();
	const { agent, remote } = useRow();
	const { source } = useServices();
	const native = useComposerNative();
	const download = useDownload(agent, path);
	if (!remote) return null;

	const button = cn(
		"inline-flex h-[22px] min-w-[22px] shrink-0 cursor-pointer items-center justify-center gap-1 rounded-md px-1 text-[11px] text-muted-foreground tabular-nums hover:bg-accent hover:text-foreground",
		className
	);

	if (download === null) {
		return (
			<button
				type="button"
				aria-label={translate("Download {{name}}", { name: name })}
				title={translate("Download to this Mac")}
				onClick={() => void startDownload(source, agent, path)}
				className={button}
			>
				<Download size={13} strokeWidth={1.9} aria-hidden />
			</button>
		);
	}

	switch (download.state) {
		case "downloading": {
			const done = percent(download.sent, download.total);
			return (
				<button
					type="button"
					aria-label={translate("Cancel downloading {{name}}", { name: name })}
					title={HELD_TITLE[download.held ?? "active"]}
					onClick={() => cancelDownload(source, agent, path)}
					className={button}
				>
					{download.held ? (
						<Pause size={11} strokeWidth={2.2} aria-hidden />
					) : (
						<Spinner size={11} />
					)}
					{done !== null ? <span>{done}%</span> : null}
				</button>
			);
		}
		case "done": {
			const { revealPath } = native;
			if (!revealPath) {
				return (
					<span
						role="img"
						aria-label={translate("Downloaded {{name}}", { name: name })}
						title={download.path}
						className={cn(
							button,
							"cursor-default text-status-ready-text hover:bg-transparent"
						)}
					>
						<Check size={13} strokeWidth={2.2} aria-hidden />
					</span>
				);
			}
			return (
				<button
					type="button"
					aria-label={translate("Show {{name}} in Finder", { name: name })}
					title={download.path}
					onClick={() => {
						const landed = download.path;
						revealPath(landed).catch(() => {
							put(keyFor(agent, path), {
								state: "failed",
								reason: translate("Couldn't find {{baseName}} in Downloads.", {
									baseName: baseName(landed),
								}),
							});
						});
					}}
					className={button}
				>
					<FolderOpen size={13} strokeWidth={1.9} aria-hidden />
				</button>
			);
		}
		case "failed":
			return (
				<>
					<span role="alert" className="sr-only">
						{download.reason}
					</span>
					<button
						type="button"
						aria-label={translate("Retry downloading {{name}}", { name: name })}
						title={download.reason}
						onClick={() => void startDownload(source, agent, path)}
						className={cn(button, "text-status-error-text")}
					>
						<RotateCw size={12} strokeWidth={2} aria-hidden />
					</button>
				</>
			);
	}
};
