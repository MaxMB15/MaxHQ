// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Android's back button and back gesture. The navigator already closes a
// sheet or steps back a screen, and on the first screen back leaves the app.
// A screen with a layer of its own open, such as a selection or an open bar,
// takes back while it's open, so back closes that layer first rather than
// leaving mid-way. iOS has no back button, so none of this runs there.

import { useIsFocused } from "@react-navigation/native";
import { useEffect, useEffectEvent } from "react";
import { BackHandler } from "react-native";

/**
 * Calls `onBack` instead of going back while `active` and the screen is the
 * one showing. A screen opened over it gets back first.
 */
export const useBackPress = (active: boolean, onBack: () => void): void => {
	const focused = useIsFocused();
	const back = useEffectEvent(onBack);
	useEffect(() => {
		if (!active || !focused) return;
		const watch = BackHandler.addEventListener("hardwareBackPress", () => {
			back();
			return true;
		});
		return () => watch.remove();
	}, [active, focused]);
};
