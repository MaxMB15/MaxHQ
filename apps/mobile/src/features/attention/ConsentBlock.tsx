// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A card that asks for your consent, laid out as the desktop's: the risk in
// plain words, what changed since you last allowed it, the details to
// judge it by, each thing it covers under its heading with its warning, a
// Show for a value that looks secret, the box that keeps the answer, and
// the buttons. It's answered only here in the app, never from a
// notification. A card the phone can't answer shows a note in place of
// the box and the buttons.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { consentAnswer, consentGroups, shownValue } from "@maxhq/shared/agent-view/consent";
import type {
	AttentionAnswer,
	Consent,
	ConsentFact,
	ConsentRow,
	Question,
} from "@maxhq/shared/bindings";
import { sentence } from "@maxhq/shared/status/format";

import { color, font, radius, size, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";

const Row = ({ row }: { row: ConsentRow }) => {
	const { t } = useI18n();

	const [shown, setShown] = useState(false);
	return (
		<View style={styles.row}>
			<View style={styles.line}>
				<Text style={styles.pair} selectable>
					{row.name}
					<Text style={styles.equals}>=</Text>
					<Text style={styles.value}>{shownValue(row, shown)}</Text>
				</Text>
				{row.secret ? (
					<Pressable
						accessibilityRole="button"
						accessibilityLabel={
							shown
								? t("Hide the value of {{name}}", { name: row.name })
								: t("Show the value of {{name}}", { name: row.name })
						}
						onPress={() => setShown(!shown)}
						hitSlop={8}
					>
						<Text style={styles.show}>{shown ? t("Hide") : t("Show")}</Text>
					</Pressable>
				) : null}
			</View>
			{row.warning ? (
				<Text style={row.strong ? styles.strong : styles.warning}>
					{sentence(row.warning)}.
				</Text>
			) : null}
		</View>
	);
};

/** The details, each a muted label over its value. */
const Facts = ({ facts }: { facts: readonly ConsentFact[] }) => {
	return (
		<View style={styles.facts}>
			{facts.map((fact, i) => (
				// A label can come more than once, and the list doesn't change.
				<View key={`${i}:${fact.label}`} style={styles.fact}>
					<Text style={styles.factLabel}>{fact.label}</Text>
					<Text style={styles.factValue} selectable>
						{fact.value}
					</Text>
				</View>
			))}
		</View>
	);
};

const RememberCheck = ({
	label,
	where,
	checked,
	onChange,
}: {
	label: string;
	where: string;
	checked: boolean;
	onChange: (checked: boolean) => void;
}) => {
	return (
		<View style={styles.remember}>
			<Pressable
				accessibilityRole="checkbox"
				accessibilityLabel={label}
				accessibilityState={{ checked }}
				onPress={() => onChange(!checked)}
				style={styles.check}
			>
				<View style={[styles.box, checked && styles.ticked]}>
					{checked ? <Text style={styles.tick}>✓</Text> : null}
				</View>
				<Text style={styles.label}>{label}</Text>
			</Pressable>
			{checked ? (
				<Text accessibilityLiveRegion="polite" style={styles.where}>
					{where}
				</Text>
			) : null}
		</View>
	);
};

export const ConsentBlock = ({
	question,
	consent,
	busy,
	onSubmit,
	note,
}: {
	question: Question;
	consent: Consent;
	busy: boolean;
	/** Sends the answer. Without it the card is read only. */
	onSubmit?: (answer: AttentionAnswer) => void;
	/** What a read-only card says in place of its buttons. */
	note?: string;
}) => {
	const { t } = useI18n();

	const [remember, setRemember] = useState(false);
	const [primary, ...others] = question.options;
	const pick = (label: string) => onSubmit?.(consentAnswer(question, label, remember));
	return (
		<View style={styles.block}>
			<Text style={styles.risk}>{consent.risk}</Text>
			{consent.changes.length > 0 ? (
				<View style={styles.group}>
					<Text style={styles.heading}>
						{t("What changed since you last allowed them")}
					</Text>
					{consent.changes.map((line) => (
						<Text key={line} style={styles.change} selectable>
							{line}
						</Text>
					))}
				</View>
			) : null}
			{consent.facts?.length ? <Facts facts={consent.facts} /> : null}
			{consent.rows.length > 0 ? (
				<View style={styles.rows}>
					{consentGroups(consent.rows).map((group) => (
						<View key={group.name} style={styles.group}>
							<Text style={styles.heading}>{group.name}</Text>
							{group.rows.map((row) => (
								<Row key={row.name} row={row} />
							))}
						</View>
					))}
				</View>
			) : null}
			{consent.remember && onSubmit ? (
				<RememberCheck
					label={consent.remember.label}
					where={consent.remember.settings}
					checked={remember}
					onChange={setRemember}
				/>
			) : null}
			{!onSubmit ? (
				note ? (
					<Text style={styles.note}>{note}</Text>
				) : null
			) : (
				<View style={styles.buttons}>
					{primary ? (
						<Button
							label={primary.label}
							tone="primary"
							grow={1}
							disabled={busy}
							onPress={() => pick(primary.label)}
						/>
					) : null}
					{others.map((option) => (
						<Button
							key={option.label}
							label={option.label}
							grow={1}
							disabled={busy}
							onPress={() => pick(option.label)}
						/>
					))}
				</View>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	block: { gap: 10 },
	risk: { color: color.foreground, fontSize: size.small },
	rows: {
		gap: 10,
		paddingHorizontal: 10,
		paddingVertical: 8,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.background,
	},
	group: { gap: 4 },
	heading: { color: color.mutedForeground, fontSize: size.tiny, fontWeight: "600" },
	change: { color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	facts: { gap: 6 },
	fact: { gap: 1 },
	factLabel: { color: color.mutedForeground, fontSize: size.tiny },
	factValue: { color: color.secondaryForeground, fontFamily: font.mono, fontSize: size.meta },
	row: { gap: 2 },
	line: { flexDirection: "row", alignItems: "flex-start", gap: 8 },
	pair: {
		flex: 1,
		minWidth: 0,
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.meta,
	},
	equals: { color: color.mutedForeground },
	value: { color: color.secondaryForeground },
	show: { color: color.primary, fontSize: size.tiny },
	warning: { color: color.busyText, fontSize: size.tiny },
	strong: { color: color.errorText, fontSize: size.tiny, fontWeight: "600" },
	remember: { gap: 2 },
	check: { minHeight: TOUCH, flexDirection: "row", alignItems: "center", gap: 10 },
	box: {
		width: 18,
		height: 18,
		borderRadius: 4,
		borderWidth: 1.5,
		borderColor: color.mutedForeground,
		alignItems: "center",
		justifyContent: "center",
	},
	ticked: { borderColor: color.primary, backgroundColor: color.primary },
	tick: { color: color.primaryForeground, fontSize: 12, fontWeight: "700" },
	label: { color: color.foreground, fontSize: size.small },
	where: { color: color.mutedForeground, fontSize: size.tiny, marginLeft: 28 },
	buttons: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	note: { color: color.connectingText, fontSize: size.small, fontWeight: "500" },
});
