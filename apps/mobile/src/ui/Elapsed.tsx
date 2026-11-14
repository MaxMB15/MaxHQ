// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A clock counting up from when something started. Only this text redraws
// each second, and it nests inside a line of text.

import { useI18n } from "@maxhq/shared/i18n";

import { Text, type TextStyle } from "react-native";

import type { Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatDuration } from "@maxhq/shared/status/format";

export const Elapsed = ({
	since,
	format = formatDuration,
	style,
}: {
	since: Timestamp;
	format?: (ms: number) => string;
	style?: TextStyle;
}) => {
	useI18n();

	const now = useNow(1000);
	return <Text style={style}>{format(Math.max(0, now - since))}</Text>;
};
