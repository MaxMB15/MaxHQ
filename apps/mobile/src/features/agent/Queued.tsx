// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The messages waiting for the turn to end, as dashed bubbles above the
// compose box, as the desktop shows them. Each can be taken back.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import { type QueuedMessage, queuedLook } from "@maxhq/shared/composer/send";

import { color, size } from "../../theme";
import { Button } from "../../ui/Button";

const Bubble = ({
	message,
	busy,
	onRemove,
}: {
	message: QueuedMessage;
	busy: boolean;
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
			<View style={styles.row}>
				<Text style={styles.meta}>{t("Queued, sends when this turn ends")}</Text>
				<Button label={t("Remove")} tone="link" small disabled={busy} onPress={onRemove} />
			</View>
		</View>
	);
};

/** The waiting messages, oldest first, or nothing when none wait. */
export const QueuedList = ({
	messages,
	pending,
	onRemove,
}: {
	messages: readonly QueuedMessage[];
	/** The messages a Remove is under way for. */
	pending: ReadonlySet<string>;
	onRemove: (message: QueuedMessage) => void;
}) => {
	return messages.length === 0 ? null : (
		<View style={styles.list}>
			{messages.map((message) => (
				<Bubble
					key={message.id}
					message={message}
					busy={pending.has(message.id)}
					onRemove={() => onRemove(message)}
				/>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	list: { gap: 8, paddingHorizontal: 4 },
	message: { alignItems: "flex-end" },
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
