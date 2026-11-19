// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The small tag on an agent's row naming its Mac, while several Macs show
// together.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, View } from "react-native";

import { Pill } from "../../ui/Pill";

export const MacTag = ({ name }: { name: string }) => {
	const { t } = useI18n();
	return (
		<View style={styles.tag} accessible accessibilityLabel={t("On {{mac}}", { mac: name })}>
			<Pill label={name} />
		</View>
	);
};

const styles = StyleSheet.create({
	tag: { maxWidth: 96, flexShrink: 1 },
});
