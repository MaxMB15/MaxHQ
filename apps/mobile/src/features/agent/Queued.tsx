// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The messages waiting for the turn to end, as dashed bubbles above the
// compose box, as the desktop shows them. Each can go into the turn now,
// steered in while it works or sent now past the prompts that wait, go
// back to the box to edit, or be taken back.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import { SKIP_WORDS } from "@maxhq/shared/agent-view/dismiss";
import type { SendMode } from "@maxhq/shared/bindings";
import { type QueuedMessage, queuedLook } from "@maxhq/shared/composer/send";

import { color, size } from "../../theme";
import { Button } from "../../ui/Button";

/** How a queued message goes into the turn now, or null while nothing runs. */
type Now = Extract<SendMode, "steer" | "interrupt"> | null;

const Bubble = ({
	message,
	now,
	busy,
	onSteer,
	onEdit,
	onRemove,
}: {
	message: QueuedMessage;
	now: Now;
	busy: boolean;
	onSteer: () => void;
	onEdit: () => void;
	onRemove: () => void;
}) => {
	const { t } = useI18n();

	const { text, files } = queuedLook(message);
	return (
		<View style={styles.message} accessibilityLabel={t("Queued message")}>
			<View style={styles.bubble}>
				<Text style={styles.text} numberOfLines={6}>
					{text}
				</Text>
				{files > 0 ? (
					<Text style={styles.meta}>{t("{{count}} attachments", { count: files })}</Text>
				) : null}
			</View>
			<Text style={styles.meta}>{t("Queued, sends when this turn ends")}</Text>
			<View style={styles.row}>
				{now !== null ? (
					<Button
						label={now === "interrupt" ? SKIP_WORDS.sendNow : t("Steer now")}
						hint={now === "interrupt" ? SKIP_WORDS.sendNowTip : undefined}
						tone="link"
						small
						disabled={busy}
						onPress={onSteer}
					/>
				) : null}
				<Button label={t("Edit")} tone="link" small disabled={busy} onPress={onEdit} />
				<Button label={t("Remove")} tone="link" small disabled={busy} onPress={onRemove} />
			</View>
		</View>
	);
};

/** The waiting messages, oldest first, or nothing when none wait. */
export const QueuedList = ({
	messages,
	now,
	pending,
	onSteer,
	onEdit,
	onRemove,
}: {
	messages: readonly QueuedMessage[];
	/** How a message goes into the turn now, or null while nothing runs. */
	now: Now;
	/** The messages an action is under way for. */
	pending: ReadonlySet<string>;
	onSteer: (message: QueuedMessage) => void;
	onEdit: (message: QueuedMessage) => void;
	onRemove: (message: QueuedMessage) => void;
}) => {
	return messages.length === 0 ? null : (
		<View style={styles.list}>
			{messages.map((message) => (
				<Bubble
					key={message.id}
					message={message}
					now={now}
					busy={pending.has(message.id)}
					onSteer={() => onSteer(message)}
					onEdit={() => onEdit(message)}
					onRemove={() => onRemove(message)}
				/>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	list: { gap: 8, paddingHorizontal: 4 },
	message: { alignItems: "flex-end", gap: 2 },
	bubble: {
		maxWidth: "85%",
		borderRadius: 12,
		borderWidth: 1,
		borderStyle: "dashed",
		borderColor: color.input,
		paddingHorizontal: 12,
		paddingVertical: 8,
		gap: 4,
	},
	text: { color: color.secondaryForeground, fontSize: size.small, lineHeight: 18 },
	row: { flexDirection: "row", alignItems: "center", gap: 2 },
	meta: { color: color.mutedForeground, fontSize: size.tiny },
});
