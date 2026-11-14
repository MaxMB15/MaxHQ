// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where a PR lives on GitHub, for the "Open on GitHub" links.

import type { PullKey } from "../bindings";

/** The PR's page on GitHub. */
export const pullUrl = (key: PullKey): string =>
	`https://github.com/${key.repo}/pull/${key.number}`;
