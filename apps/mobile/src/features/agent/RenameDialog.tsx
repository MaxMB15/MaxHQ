// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The small dialog that renames an agent: its name in a box, selected so
// typing replaces it, with Cancel and Save under it. A blank name brings
// the agent's own title back, and an alert says why maxd refused.

import { useI18n } from "@maxhq/shared/i18n";

import { type ComponentRef, useRef } from "react";
import { Alert, KeyboardAvoidingView, Modal, StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import { Button } from "../../ui/Button";
import { Input } from "../../ui/Field";
import { color, radius, size } from "../../theme";
import type { RenameDialog as Dialog } from "./more";

export const RenameDialog = ({ dialog }: { dialog: Dialog }) => {
	const { t } = useI18n();

	const open = useStore(dialog.store, (s) => s.agent !== null);
	const draft = useStore(dialog.store, (s) => s.draft);
	const input = useRef<ComponentRef<typeof Input>>(null);
	const save = () =>
		void dialog.save().then((why) => {
			if (why !== null) Alert.alert(why);
		});

	return (
		<Modal
			visible={open}
			transparent
			animationType="fade"
			onShow={() => input.current?.focus()}
			onRequestClose={dialog.cancel}
		>
			<KeyboardAvoidingView behavior="padding" style={styles.scrim}>
				<View style={styles.card} accessibilityViewIsModal>
					<Text style={styles.title} accessibilityRole="header">
						{t("Rename agent")}
					</Text>
					<Input
						ref={input}
						value={draft}
						onChangeText={dialog.edit}
						accessibilityLabel={t("Agent name")}
						selectTextOnFocus
						autoCorrect={false}
						returnKeyType="done"
						onSubmitEditing={save}
					/>
					<View style={styles.buttons}>
						<Button label={t("Cancel")} grow={1} onPress={dialog.cancel} />
						<Button label={t("Save")} tone="primary" grow={1} onPress={save} />
					</View>
				</View>
			</KeyboardAvoidingView>
		</Modal>
	);
};

const styles = StyleSheet.create({
	scrim: {
		flex: 1,
		justifyContent: "center",
		padding: 24,
		backgroundColor: "rgba(0, 0, 0, 0.55)",
	},
	card: {
		gap: 14,
		padding: 16,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.card,
	},
	title: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	buttons: { flexDirection: "row", gap: 10 },
});
