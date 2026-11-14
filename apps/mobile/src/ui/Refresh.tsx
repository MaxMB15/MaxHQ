// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pull to refresh: the spinner shows until the
// refresh settles, and a pull while one runs starts nothing more.

import { useRef, useState } from "react";
import { RefreshControl } from "react-native";

import { color } from "../theme";

/** A list's `refreshControl` that runs `refresh` on a pull. */
export const usePullToRefresh = (refresh: () => Promise<unknown>) => {
	const [refreshing, setRefreshing] = useState(false);
	const running = useRef(false);
	const onRefresh = () => {
		if (running.current) return;
		running.current = true;
		setRefreshing(true);
		refresh()
			.catch((error: unknown) => console.warn("Couldn't refresh", error))
			.finally(() => {
				running.current = false;
				setRefreshing(false);
			});
	};
	return (
		<RefreshControl
			refreshing={refreshing}
			onRefresh={onRefresh}
			tintColor={color.mutedForeground}
		/>
	);
};
