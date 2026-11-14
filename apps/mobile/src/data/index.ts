// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one switch between sample data and the phone's core. A build uses the
// core unless EXPO_PUBLIC_MAXHQ_DATA is "mock". The core opens on the app's
// documents folder, with its media cache in the app's caches folder, which
// the system may empty when space runs low. Opening it again after a JS
// reload lets go of what the old page watched.

import { Paths } from "expo-file-system";
import * as core from "maxhq-core";

import { mockSource } from "@maxhq/shared/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";

import { nativeSource, pathOf, phoneCalls, type PhoneCalls } from "./native-source";

/** The source this build uses, and the phone's own calls when it's the core. */
export const createSource = (): { source: DataSource; phone: PhoneCalls | null } => {
	if (process.env.EXPO_PUBLIC_MAXHQ_DATA === "mock") return { source: mockSource(), phone: null };
	const dir = `${pathOf(Paths.document.uri)}/maxhq`;
	const cache = `${pathOf(Paths.cache.uri)}/maxhq`;
	const phone = phoneCalls(core);
	phone.start(dir, cache);
	return { source: nativeSource(core, cache), phone };
};
