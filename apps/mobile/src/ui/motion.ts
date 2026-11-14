// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the phone is set to reduce motion, so a view jumps where it would
// slide.

import { useEffect, useState } from "react";
import { AccessibilityInfo } from "react-native";

export const useReduceMotion = (): boolean => {
	const [reduce, setReduce] = useState(false);
	useEffect(() => {
		let live = true;
		void AccessibilityInfo.isReduceMotionEnabled().then((on) => {
			if (live) setReduce(on);
		});
		const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setReduce);
		return () => {
			live = false;
			sub.remove();
		};
	}, []);
	return reduce;
};
