// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Time in the background for an answer from a notification's buttons, from
// the app's own module in modules/maxhq-background. A build from before
// the module has none, and the answer gets what iOS gives.

import { requireOptionalNativeModule } from "expo";

type Background = { hold(): Promise<void>; release(): Promise<void> };

const native = requireOptionalNativeModule<Background>("MaxhqBackground");

const quietly = async (call: (() => Promise<void>) | undefined) => {
	try {
		await call?.();
	} catch (error: unknown) {
		console.warn("Couldn't change the background time", error);
	}
};

/** Runs `work` with the app kept running in the background until it ends. */
export const withBackgroundTime = async <T>(work: () => Promise<T>): Promise<T> => {
	await quietly(native ? () => native.hold() : undefined);
	try {
		return await work();
	} finally {
		void quietly(native ? () => native.release() : undefined);
	}
};
