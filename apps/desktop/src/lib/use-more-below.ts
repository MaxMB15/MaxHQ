// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A box that scrolls, and whether more of its text sits below what it shows,
// kept up as it scrolls and resizes, so what sits past the fold can't go
// unread.

import { useCallback, useLayoutEffect, useRef, useState } from "react";

export const useMoreBelow = <T extends HTMLElement>(text: string | null) => {
	const ref = useRef<T>(null);
	const [more, setMore] = useState(false);
	const measure = useCallback(() => {
		const box = ref.current;
		setMore(box !== null && box.scrollHeight - box.clientHeight - box.scrollTop > 1);
	}, []);
	useLayoutEffect(() => {
		measure();
		const box = ref.current;
		if (box === null) return;
		const observer = new ResizeObserver(measure);
		observer.observe(box);
		return () => observer.disconnect();
	}, [measure, text]);
	return { ref, more, measure };
};
