// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Fetching files a remote agent can see, as the desktop's download button
// and the phone's Save do. Each download is kept by the agent and the path,
// outside any view, so one keeps going while its row scrolls away or its
// screen closes, and a second ask for the same file joins the first.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentId, TransferId } from "../bindings";
import type { DataSource, DownloadResult } from "../data/source";
import { errorText } from "../timeline/store";

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
			/** Where it landed on this device. */
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
	source: Pick<DataSource, "download" | "cancelTransfer">,
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
export const cancelDownload = (
	source: Pick<DataSource, "cancelTransfer">,
	agent: AgentId,
	path: string
): void => {
	const key = keyFor(agent, path);
	const job = jobs.get(key);
	if (job) {
		job.cancelled = true;
		jobs.delete(key);
		if (job.transfer) void source.cancelTransfer(job.transfer);
	}
	put(key, null);
};

/** Marks a finished download as failed, such as one whose file has gone since. */
export const failDownload = (agent: AgentId, path: string, reason: string): void => {
	const key = keyFor(agent, path);
	if (!jobs.has(key)) put(key, { state: "failed", reason });
};

/** Where a download of this file stands now, or null when there's none. */
export const downloadOf = (agent: AgentId, path: string): DownloadState | null =>
	store.getState().downloads.get(keyFor(agent, path)) ?? null;

/** Where a download of this file stands, or null when there's none. */
export const useDownload = (agent: AgentId, path: string): DownloadState | null =>
	useStore(store, (s) => s.downloads.get(keyFor(agent, path)) ?? null);

/** How far a download has got, 0 to 100, or null while its size is unknown. */
export const downloadPercent = (sent: number, total: number): number | null =>
	total > 0 ? Math.min(100, Math.floor((sent / total) * 100)) : null;

/** Forgets every download, for tests. */
export const clearDownloads = (): void => {
	for (const job of jobs.values()) job.cancelled = true;
	jobs.clear();
	store.setState({ downloads: new Map() });
};
