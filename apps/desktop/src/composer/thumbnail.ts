// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A picture of a file on this Mac, for a pill's icon and its preview.

import { useEffect, useState } from "react";

import type { ComposerNative } from "./native";

/**
 * A thumbnail's object URL for a dropped, pasted or picked path, or null
 * while it loads and when there's none. Revoked when the path changes and
 * on unmount.
 */
export const useThumbnail = (
	native: ComposerNative,
	path: string | null,
	size: number
): string | null => {
	const [shown, setShown] = useState<{ path: string; url: string | null } | null>(null);
	useEffect(() => {
		if (path === null) return;
		let live = true;
		let url: string | null = null;
		native.thumbnail(path, size).then(
			(png) => {
				if (!live) return;
				url = png ? URL.createObjectURL(png) : null;
				setShown({ path, url });
			},
			() => {
				if (live) setShown({ path, url: null });
			}
		);
		return () => {
			live = false;
			if (url !== null) URL.revokeObjectURL(url);
		};
	}, [native, path, size]);
	return shown !== null && shown.path === path ? shown.url : null;
};
