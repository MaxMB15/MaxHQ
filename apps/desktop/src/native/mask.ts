// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A command's output with the keys and tokens MaxHQ recognizes masked, the
// way the backend masks it before an agent reads it. The browser build has
// no backend, so it shows the text as it is.

import { useEffect, useState } from "react";

import { commands } from "@/data/tauri-commands";

import { inApp } from "./platform";

/** `text` as an agent would read it. */
export const maskOutput = (text: string): Promise<string> =>
	inApp() ? commands.maskOutput(text) : Promise.resolve(text);

/** `text` masked, or null until the backend answers. */
export const useMasked = (text: string): string | null => {
	const [masked, setMasked] = useState<{ readonly from: string; readonly text: string } | null>(
		null
	);
	useEffect(() => {
		let live = true;
		void maskOutput(text).then((result) => {
			if (live) setMasked({ from: text, text: result });
		});
		return () => {
			live = false;
		};
	}, [text]);
	return masked?.from === text ? masked.text : null;
};
