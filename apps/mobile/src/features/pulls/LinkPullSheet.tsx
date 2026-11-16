// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The small dialog that links a PR to an agent by its number, as the
// desktop's link field does: the number in a box, Cancel and Link under
// it, and a line that says why the device refused one. The agent's chips
// show the PR once its device links it.

import { useI18n } from "@maxhq/shared/i18n";

import { type ComponentRef, useRef, useState } from "react";
import { KeyboardAvoidingView, Modal, StyleSheet, Text, View } from "react-native";

import type { Agent } from "@maxhq/shared/bindings";
import { linkPullNumber, parsePullNumber } from "@maxhq/shared/pulls/chip";

import { useServices } from "../../services";
import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Field";

export const LinkPullSheet = ({
	agent,
	open,
	onClose,
}: {
	agent: Pick<Agent, "id">;
	open: boolean;
	onClose: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const input = useRef<ComponentRef<typeof Input>>(null);
	const [typed, setTyped] = useState("");
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);
	const number = parsePullNumber(typed);

	const close = () => {
		setTyped("");
		setFailure(null);
		onClose();
	};
	const link = async () => {
		if (number === null || busy) return;
		setBusy(true);
		setFailure(null);
		try {
			const why = await linkPullNumber(source, agent.id, number);
			if (why) setFailure(why);
			else close();
		} finally {
			setBusy(false);
		}
	};

	return (
		<Modal
			visible={open}
			transparent
			animationType="fade"
			onShow={() => input.current?.focus()}
			onRequestClose={close}
		>
			<KeyboardAvoidingView behavior="padding" style={styles.scrim}>
				<View style={styles.card} accessibilityViewIsModal>
					<Text style={styles.title} accessibilityRole="header">
						{t("Link a pull request")}
					</Text>
					<Input
						ref={input}
						value={typed}
						onChangeText={(text) => {
							setTyped(text);
							setFailure(null);
						}}
						placeholder={t("Such as 142")}
						accessibilityLabel={t("Pull request number")}
						keyboardType="number-pad"
						autoCorrect={false}
						returnKeyType="done"
						onSubmitEditing={() => void link()}
					/>
					{failure ? (
						<Text style={styles.failure} accessibilityRole="alert">
							{failure}
						</Text>
					) : (
						<Text style={styles.note}>
							{t("A PR in the repository the agent works in.")}
						</Text>
					)}
					<View style={styles.buttons}>
						<Button label={t("Cancel")} grow={1} onPress={close} />
						<Button
							label={busy ? t("Linking") : t("Link")}
							tone="primary"
							grow={1}
							disabled={number === null || busy}
							onPress={() => void link()}
						/>
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
	note: { color: color.mutedForeground, fontSize: size.tiny },
	failure: { color: color.errorText, fontSize: size.tiny },
	buttons: { flexDirection: "row", gap: 10 },
});
