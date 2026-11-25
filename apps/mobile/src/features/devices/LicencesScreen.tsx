// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The licences a device's installs wait on, each in full with Accept, as
// the desktop's dialog shows them. Accept sends the text's SHA-256 as
// read, so a licence that changed since asks again rather than being
// accepted unseen. Accepting works from any phone, without the Mac's switch.

import { useI18n } from "@maxhq/shared/i18n";

import { memo } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import { type LicenceAsk, pendingLicences, toolName } from "@maxhq/shared/hosts/tools";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Header } from "../../ui/Header";
import { Buttons, NoteLine, useAction } from "./section";

const Licence = memo(function Licence({ ask }: { ask: LicenceAsk }) {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	const { device, licence, tool } = ask;
	return (
		<View style={styles.licence} accessibilityLabel={licence.id}>
			<Text accessibilityRole="header" style={styles.id}>
				{licence.id}{" "}
				<Text style={styles.for}>
					{t("for {{value2}}", { value2: toolName(tool.kind) })}
				</Text>
			</Text>
			<Text
				selectable
				accessibilityLabel={t("{{id}} text", { id: licence.id })}
				style={styles.text}
			>
				{licence.text}
			</Text>
			<Buttons>
				<Button
					label={t("Accept")}
					hint={t("Accept {{id}}", { id: licence.id })}
					tone="primary"
					grow={1}
					disabled={busy}
					onPress={() =>
						void run(t("Sending"), () =>
							source.acceptLicence(device, licence.id, licence.text_sha256)
						)
					}
				/>
			</Buttons>
			<NoteLine note={note} />
		</View>
	);
});

export const LicencesScreen = ({ navigation, route }: ScreenProps<"Licences">) => {
	const { t } = useI18n();

	const { device } = route.params;
	const all = useView((s) => s.view);
	const asks = pendingLicences(all).filter((ask) => ask.device === device);
	const close = () => navigation.goBack();
	return (
		<View style={styles.screen}>
			<Header
				title={t("Licences to accept")}
				sheet
				back={{ label: t("Close"), onPress: close, close: true }}
			/>
			<ScrollView contentContainerStyle={styles.content}>
				<Text style={styles.body}>
					{t(
						"The install goes on once you accept each one. MaxHQ keeps which text you accepted, and asks again if it changes."
					)}
				</Text>
				{asks.length === 0 ? (
					<Text style={styles.empty}>{t("Nothing waits on a licence.")}</Text>
				) : (
					asks.map((ask) => (
						<Licence
							key={`${ask.licence.id}-${ask.tool.place.type === "host" ? "host" : ask.tool.place.workspace}`}
							ask={ask}
						/>
					))
				)}
				<Buttons>
					<Button label={t("Close")} grow={1} onPress={close} />
				</Buttons>
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 16, paddingBottom: 40 },
	body: { color: color.secondaryForeground, fontSize: size.small, lineHeight: 19 },
	empty: { color: color.mutedForeground, fontSize: size.small },
	licence: { gap: 8 },
	id: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	for: { color: color.mutedForeground, fontWeight: "400" },
	text: {
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
		paddingHorizontal: 10,
		paddingVertical: 8,
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.tiny,
	},
});
