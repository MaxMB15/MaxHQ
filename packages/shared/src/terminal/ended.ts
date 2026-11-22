// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a command you ran with `!` ended, in the words every card and row
// uses: its exit code, "Stopped after 33 s" when your Stop or Force stop
// ended it, or the signal that did, named when it's a common one.

import { t } from "../i18n";

import type { TerminalStatus } from "../bindings";
import { formatTook } from "../agent-view/format";
import { formatDuration } from "../status/format";

/** A terminal's status once what ran in it ended. */
export type ExitedStatus = Extract<TerminalStatus, { state: "exited" }>;

/** The name of a common signal, such as "killed" for 9, or null for any other. */
export const signalName = (signal: number): string | null => {
	switch (signal) {
		case 1:
			return t("hangup");
		case 2:
			return t("interrupt");
		case 3:
			return t("quit");
		case 4:
			return t("illegal instruction");
		case 6:
			return t("abort");
		case 8:
			return t("floating point error");
		case 9:
			return t("killed");
		case 11:
			return t("segmentation fault");
		case 13:
			return t("broken pipe");
		case 14:
			return t("alarm");
		case 15:
			return t("terminated");
		default:
			return null;
	}
};

/**
 * How a command ended, with how long it took when that's known: "exit 1 ·
 * 4.2 s", "Stopped after 33 s" when you stopped it, or "Ended by signal 9
 * (killed) · 4.2 s". A `maxd` older than protocol 59 never says you
 * stopped it, so its Stop reads as the signal that ended the command.
 */
export const endedText = (status: ExitedStatus, took: number | null): string => {
	if (status.stopped === true)
		return took === null
			? t("Stopped")
			: t("Stopped after {{time}}", { time: formatDuration(took) });
	const how =
		status.code !== null
			? t("exit {{code}}", { code: status.code })
			: status.signal !== null
				? signalWords(status.signal)
				: t("Stopped");
	return took === null ? how : `${how} · ${formatTook(took)}`;
};

const signalWords = (signal: number): string => {
	const name = signalName(signal);
	return name === null
		? t("Ended by signal {{signal}}", { signal })
		: t("Ended by signal {{signal}} ({{name}})", { signal, name });
};
