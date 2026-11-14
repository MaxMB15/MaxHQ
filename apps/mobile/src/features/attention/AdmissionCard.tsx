// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's card about a program that connected without a key it trusts
// and asked for more, in the inbox. It lays out what the device knows, as
// the Mac's does, but the phone can't answer it: only a desktop app that
// proved its key lets things in, so it says to answer on the Mac.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import type { Admission } from "@maxhq/shared/bindings";
import { ANSWER_ON_THE_MAC, admissionCard } from "@maxhq/shared/ui/admission";
import type { ViewState } from "@maxhq/shared/view/replica";

import { color, radius, size, tint } from "../../theme";
import { ConsentBlock } from "./ConsentBlock";

export const AdmissionCard = ({ admission, view }: { admission: Admission; view: ViewState }) => {
	const { t } = useI18n();

	const card = admissionCard(view, admission);
	const consent = admission.question.consent ?? {
		title: card.title,
		risk: card.risk,
		changes: [],
		facts: [...card.facts],
		rows: [],
		remember: null,
	};
	return (
		<View
			style={styles.card}
			accessibilityLabel={t("Program asking to control {{device}}", {
				device: card.deviceName,
			})}
		>
			<View style={styles.head}>
				<Text style={styles.title} numberOfLines={2}>
					{card.title}
				</Text>
			</View>
			<Text style={styles.where}>{t("On {{device}}.", { device: card.deviceName })}</Text>
			<ConsentBlock
				question={admission.question}
				consent={{ ...consent, remember: null }}
				busy={false}
				note={ANSWER_ON_THE_MAC}
			/>
		</View>
	);
};

const styles = StyleSheet.create({
	card: {
		gap: 8,
		marginBottom: 12,
		padding: 10,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.connecting, 0.4),
		backgroundColor: color.card,
	},
	head: { flexDirection: "row", alignItems: "center", gap: 9 },
	title: {
		flex: 1,
		minWidth: 0,
		color: color.foregroundStrong,
		fontSize: size.body,
		fontWeight: "600",
	},
	where: { color: color.mutedForeground, fontSize: size.tiny },
});
