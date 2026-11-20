// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The notification log calls of the sample data and of tests. The Mac and
// devbox each keep a made-up log of the alerts their agents raised over
// the last day, one of them about an agent that's gone since. Reads and
// Clear last until the app reloads.

import type { AgentId, DeviceId, Notice, NoticeKind, Timestamp } from "../bindings";
import type { NoticeCalls } from "../notices/requests";
import { DEVBOX, MAC } from "./mock-view";

const MIN = 60_000;

/** The notification log calls of a `maxd` from before the log. */
export const noNotices: NoticeCalls = {
	listNotices: async () => ({ status: "ok", data: { notices: [], total: 0, unread: 0 } }),
	readNotices: async () => ({ status: "ok", data: null }),
	clearNotices: async () => ({ status: "ok", data: null }),
};

type Sample = readonly [AgentId, string, DeviceId, string | null, NoticeKind];

const SAMPLES: readonly Sample[] = [
	["agent-release", "Release checklist", MAC, "MaxHQ", "permission"],
	["agent-sync-drift", "Subtitle sync drift", DEVBOX, "Comet", "question"],
	["agent-usage-popover", "Usage meter popover", MAC, "MaxHQ", "finished"],
	["agent-egress", "Egress allowlist audit", DEVBOX, "DuskTools", "error"],
	["agent-hdr", "HDR tone mapping", MAC, "Comet", "finished"],
	["agent-gtfs", "GTFS feed refresh", DEVBOX, "AtlasMaps", "limit"],
	["agent-gtfs", "GTFS feed refresh", DEVBOX, "AtlasMaps", "resumed"],
	["agent-lesson-audio", "Lesson audio pipeline", DEVBOX, "Mosaic", "input"],
	["agent-gone-migration", "Schema migration dry run", DEVBOX, "Mosaic", "finished"],
];

/** The sample logs: 27 notices, the newest few unread. */
const sampleNotices = (now: Timestamp): Notice[] =>
	Array.from({ length: 27 }, (_, n) => {
		const [agent, title, device, project, kind] = SAMPLES[n % SAMPLES.length] as Sample;
		const at = now - (n + 1) * (n + 1) * 2 * MIN;
		return {
			id: 100 - n,
			device,
			agent,
			kind,
			title,
			project,
			summary: null,
			at,
			read_at: n < 4 ? null : at + MIN,
		};
	});

/** The notification log calls over the sample logs. */
export const mockNotices = (now: () => Timestamp): NoticeCalls => {
	let notices = sampleNotices(now());
	return {
		listNotices: async (limit) => ({
			status: "ok",
			data: {
				notices: notices.slice(0, limit),
				total: notices.length,
				unread: notices.filter((notice) => notice.read_at === null).length,
			},
		}),
		readNotices: async (agent) => {
			const at = now();
			notices = notices.map((notice) =>
				notice.read_at === null && (agent === null || notice.agent === agent)
					? { ...notice, read_at: at }
					: notice
			);
			return { status: "ok", data: null };
		},
		clearNotices: async () => {
			notices = [];
			return { status: "ok", data: null };
		},
	};
};
