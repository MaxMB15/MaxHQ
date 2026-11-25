// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A short list of choices in the system's own sheet. Android's alert holds
// three buttons at most, so it shows two choices at a time with More for
// the rest, and leaves out the ones you can't pick. Back or a tap outside
// closes it as Cancel would. `confirm` asks before something that can't be
// undone, and `answer` asks between two answers.

import { t } from "@maxhq/shared/i18n";

import { ActionSheetIOS, Alert, Platform } from "react-native";

export type Choice = {
	readonly label: string;
	readonly pick: () => void;
	/** Red, for one that throws something away. */
	readonly destructive?: boolean;
	/** Greyed out, such as the one already picked. */
	readonly disabled?: boolean;
};

/** Shows `choices` under `title` and `message`; a pick runs its `pick`, Cancel nothing. */
export const choose = (title: string, choices: readonly Choice[], message?: string): void => {
	if (Platform.OS === "ios") {
		ActionSheetIOS.showActionSheetWithOptions(
			{
				title,
				message,
				options: [...choices.map((c) => c.label), t("Cancel")],
				cancelButtonIndex: choices.length,
				destructiveButtonIndex: choices.flatMap((c, i) => (c.destructive ? [i] : [])),
				disabledButtonIndices: choices.flatMap((c, i) => (c.disabled ? [i] : [])),
			},
			(index) => choices[index]?.pick()
		);
		return;
	}
	alertPage(
		title,
		message,
		choices.filter((c) => !c.disabled)
	);
};

/** Asks `question` with Cancel and a red `action`; a tap on `action` runs `run`. */
export const confirm = (
	question: { readonly title: string; readonly detail: string },
	action: string,
	run: () => void
): void =>
	Alert.alert(
		question.title,
		question.detail,
		[
			{ text: t("Cancel"), style: "cancel" },
			{ text: action, style: "destructive", onPress: run },
		],
		{ cancelable: true }
	);

/**
 * Asks `question` with its `other` answer and a red `action`, and gives
 * which you tapped, or null when Android's alert is dismissed.
 */
export const answer = (question: {
	readonly title: string;
	readonly detail: string;
	readonly action: string;
	readonly other: string;
}): Promise<"action" | "other" | null> =>
	new Promise((resolve) =>
		Alert.alert(
			question.title,
			question.detail,
			[
				{ text: question.other, onPress: () => resolve("other") },
				{ text: question.action, style: "destructive", onPress: () => resolve("action") },
			],
			{ cancelable: true, onDismiss: () => resolve(null) }
		)
	);

const alertPage = (title: string, message: string | undefined, choices: readonly Choice[]) =>
	Alert.alert(
		title,
		message,
		[
			...choices.slice(0, 2).map((c) => ({
				text: c.label,
				style: c.destructive ? ("destructive" as const) : undefined,
				onPress: c.pick,
			})),
			choices.length > 2
				? { text: t("More"), onPress: () => alertPage(title, message, choices.slice(2)) }
				: { text: t("Cancel"), style: "cancel" as const },
		],
		{ cancelable: true }
	);
