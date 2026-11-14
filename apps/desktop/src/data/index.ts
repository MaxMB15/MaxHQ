// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one switch between sample data and the backend.

import { isTauri } from "@tauri-apps/api/core";

import { mockSource } from "./mock-source";
import { pickSource, type DataSource } from "@maxhq/shared/data/source";
import { tauriSource } from "./tauri-source";

/** The source this build uses. Set `VITE_MAXHQ_DATA` to "tauri" or "mock" to override it. */
export const createSource = (): DataSource =>
	pickSource(import.meta.env.VITE_MAXHQ_DATA, import.meta.env.DEV, isTauri()) === "mock"
		? mockSource()
		: tauriSource();

export type { DataSource } from "@maxhq/shared/data/source";
