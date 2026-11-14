// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

/// <reference types="vite/client" />

interface ImportMetaEnv {
	/**
	 * Where the view comes from: "tauri" for the app's backend or "mock" for
	 * sample data. Unset means sample data in dev and the backend in a build.
	 */
	readonly VITE_MAXHQ_DATA?: string;
}

interface ImportMeta {
	readonly env: ImportMetaEnv;
}
