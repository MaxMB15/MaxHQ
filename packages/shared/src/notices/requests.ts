// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The notification log's requests, as calls of a data source. Each `maxd`
// keeps the alerts about its own agents; the app asks every connected
// device and merges their pages by time. A `maxd` too old to keep them is
// left out.

import type { AgentId, NoticePage } from "../bindings";
import type { Ask, Result } from "../data/request-source";
import type { CommandResult } from "../data/source";

/** The notification log calls of a data source. */
export type NoticeCalls = {
	/** The newest `limit` notices across every device, newest first. */
	readonly listNotices: (limit: number) => Promise<Result<NoticePage>>;
	/** Marks the agent's notices read, or every notice with none. */
	readonly readNotices: (agent: AgentId | null) => Promise<CommandResult>;
	/** Forgets every device's notices. */
	readonly clearNotices: () => Promise<CommandResult>;
};

/** The notification log calls, each one request over `ask`. */
export const noticeCalls = (ask: Ask): NoticeCalls => ({
	listNotices: (limit) => ask({ kind: "list_notices", limit }),
	readNotices: (agent) => ask({ kind: "read_notices", agent }),
	clearNotices: () => ask({ kind: "clear_notices" }),
});
