// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The shared sample source, which inside the app also shows the backend's
// version.

import { isTauri } from "@tauri-apps/api/core";

import {
	mockSource as sharedMockSource,
	type MockOptions,
	type MockSource,
} from "@maxhq/shared/data/mock-source";
import { appInfo } from "./tauri-source";

/** The sample source. Inside the app it asks the backend for the version. */
export const mockSource = (options: MockOptions = {}): MockSource =>
	sharedMockSource({ appInfo: async () => (isTauri() ? appInfo() : null), ...options });
