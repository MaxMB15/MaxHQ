// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What an agent's box holds besides its text: the files staged for it and
// the pull requests and issues its `#123`s show as. They stay with the
// agent while the app runs, as its draft does, so leaving the agent and
// coming back finds its files still ready. An agent that moved to another
// device starts its files afresh there.

import type { AgentId, DeviceId } from "@maxhq/shared/bindings";
import { createStaging, type Staging } from "@maxhq/shared/composer/staging";
import type { DataSource } from "@maxhq/shared/data/source";
import { createGitPills, gitFinder, type GitPillsStore } from "@maxhq/shared/pills/git-pills";

/** An agent's staged files and its pull requests and issues. */
export type Box = {
	readonly device: DeviceId;
	readonly staging: Staging;
	readonly gitPills: GitPillsStore;
};

const boxes = new WeakMap<DataSource, Map<AgentId, Box>>();

/** The box of `agent` on `device`, made the first time it's asked for. */
export const boxOf = (source: DataSource, agent: AgentId, device: DeviceId): Box => {
	let mine = boxes.get(source);
	if (!mine) {
		mine = new Map();
		boxes.set(source, mine);
	}
	const known = mine.get(agent);
	if (known?.device === device) return known;
	const box: Box = {
		device,
		staging: createStaging(source, device, agent),
		gitPills: known?.gitPills ?? createGitPills(gitFinder(source, agent)),
	};
	mine.set(agent, box);
	return box;
};
