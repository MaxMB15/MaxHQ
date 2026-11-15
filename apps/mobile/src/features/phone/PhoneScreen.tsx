// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Settings, in cards as the desktop's: this phone's name and Unpair, what
// its send button does while an agent works, which pushes it gets, the way
// to its devices and to pairing, and, by hand, the line that lets it into a
// device and a form to add one. Opened from the first screen's "Add a
// device by hand", the form starts open and in view.

import { type LanguageChoice, useI18n } from "@maxhq/shared/i18n";

import Constants from "expo-constants";
import { type ComponentRef, type Ref, useRef, useState } from "react";
import { Alert, ScrollView, Share, StyleSheet, Text, View } from "react-native";

import { describe } from "@maxhq/shared/data/request-source";
import { useNow } from "@maxhq/shared/lib/clock";
import { deviceReach } from "@maxhq/shared/status/reach";

import type { PhoneCalls } from "../../data/native-source";
import type { ScreenProps } from "../../navigation";
import { PushSection } from "../../push/PushSection";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose } from "../../ui/choose";
import { Card, CardRow } from "../../ui/Card";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { SendSection } from "../agent/SendSection";
import { pairedWith, reachSummary } from "../devices/summary";
import { DispatcherSection } from "../dispatcher/DispatcherSection";

// Shown when the device turns the unpair away. The phone can't take itself
// out of a device's book yet, so the Mac does it.
const UNPAIR_ON_MAC =
	"This phone can't unpair itself yet. Remove it on the Mac, in Settings, Phones.";

export const PhoneScreen = ({ navigation, route }: ScreenProps<"Settings">) => {
	const { t } = useI18n();

	const { phone, source } = useServices();
	const [key] = useState(() => phone?.phoneKey());
	const id = key?.status === "ok" ? key.data.phone : null;
	const named = useView((s) => (id ? s.view.tables.phone.get(id)?.name : undefined));
	const [unpair, setUnpair] = useState<string | null>(null);
	const [byHand, setByHand] = useState(route.params?.byHand === true);
	const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
	const form = useRef<ComponentRef<typeof View>>(null);
	const jump = useRef(byHand);
	const view = useView((s) => s.view);
	const now = useNow(10_000);
	const devices = deviceReach(view, now);
	const header = (
		<Header
			title={t("Settings")}
			back={{ label: t("Back"), onPress: () => navigation.goBack() }}
		/>
	);

	if (!phone || !key) {
		return (
			<View style={styles.screen}>
				{header}
				<ScrollView contentContainerStyle={styles.content}>
					<Text style={styles.note}>{t("The sample data has no phone.")}</Text>
					<LanguageSection />
					<SendSection />
					<DispatcherSection />
				</ScrollView>
			</View>
		);
	}

	const confirmUnpair = () => {
		if (!id) return;
		Alert.alert(
			t("Unpair this phone?"),
			t("It stops reaching your devices until you pair again."),
			[
				{ text: t("Cancel"), style: "cancel" },
				{
					text: t("Unpair"),
					style: "destructive",
					onPress: () =>
						void source.removePhone(id).then(
							(result) =>
								setUnpair(
									result.status === "ok" ? t("Unpaired.") : t(UNPAIR_ON_MAC)
								),
							() => setUnpair(t(UNPAIR_ON_MAC))
						),
				},
			]
		);
	};

	// Scrolls to the form once, when the screen opened with it.
	const toForm = () => {
		const content = scroll.current?.getInnerViewRef();
		if (!jump.current || !content) return;
		jump.current = false;
		form.current?.measureLayout(content, (_x, y) => scroll.current?.scrollTo({ y }));
	};

	const version = Constants.expoConfig?.version;
	return (
		<View style={styles.screen}>
			{header}
			<ScrollView
				ref={scroll}
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				automaticallyAdjustKeyboardInsets
			>
				<Card title={t("This phone")}>
					<CardRow
						label={t("Name")}
						note={
							named
								? t("How your devices list this phone. Rename it on the Mac.")
								: undefined
						}
					>
						<Text style={styles.value} numberOfLines={1}>
							{named ?? t("Not paired yet")}
						</Text>
					</CardRow>
					{named ? (
						<CardRow label={t("Paired with")} note={pairedWith(devices) ?? undefined}>
							<Button
								label={t("Unpair")}
								tone="danger"
								small
								onPress={confirmUnpair}
							/>
						</CardRow>
					) : null}
					{unpair ? <Text style={[styles.note, styles.inCard]}>{unpair}</Text> : null}
				</Card>
				<LanguageSection />
				<SendSection />
				<PushSection />
				<DispatcherSection />
				<Card title={t("Devices")}>
					<CardRow
						label={t("Devices")}
						note={devices.length > 0 ? reachSummary(devices) : undefined}
						onPress={() => navigation.navigate("Devices")}
					/>
					<CardRow
						label={t("Pair with a device")}
						onPress={() => navigation.navigate("Scan")}
					/>
					<CardRow
						label={t("Add a device by hand")}
						note={t("For a device without the Mac nearby")}
						onPress={() => setByHand((open) => !open)}
					/>
					{byHand ? (
						<ByHand ref={form} phone={phone} line={key} onLayout={toForm} />
					) : null}
				</Card>
				{version ? <Text style={styles.version}>MaxHQ {version}</Text> : null}
			</ScrollView>
		</View>
	);
};

const LANGUAGE_NAMES: Record<LanguageChoice, string> = {
	system: "System",
	en: "English",
	nl: "Nederlands",
	de: "Deutsch",
	fr: "Français",
	es: "Español",
};

const LanguageSection = () => {
	const { t, choice, setLanguageChoice } = useI18n();
	const label = (value: LanguageChoice) =>
		value === "system" ? t("System") : t(LANGUAGE_NAMES[value]);
	return (
		<Card title={t("Language")}>
			<CardRow
				label={t("Interface language")}
				onPress={() =>
					choose(
						t("Interface language"),
						(Object.keys(LANGUAGE_NAMES) as LanguageChoice[]).map((value) => ({
							label: label(value),
							disabled: choice === value,
							pick: () => setLanguageChoice(value),
						}))
					)
				}
			>
				<Text style={styles.value}>{label(choice)}</Text>
			</CardRow>
		</Card>
	);
};

const ByHand = ({
	ref,
	phone,
	line,
	onLayout,
}: {
	ref: Ref<ComponentRef<typeof View>>;
	phone: PhoneCalls;
	line: ReturnType<PhoneCalls["phoneKey"]>;
	onLayout: () => void;
}) => {
	const { t } = useI18n();

	const [name, setName] = useState("");
	const [route, setRoute] = useState("");
	const [hostKey, setHostKey] = useState("");
	const [note, setNote] = useState<string | null>(null);

	const add = () => {
		const result = phone.addDevice(name, route, hostKey);
		setNote(
			result.status === "ok"
				? t("Added {{name}}.", { name: name.trim() })
				: describe(result.error)
		);
	};

	return (
		<View ref={ref} style={styles.byHand} onLayout={onLayout}>
			<View style={styles.section}>
				<Text style={styles.heading}>{t("Let this phone in by hand")}</Text>
				<Text style={styles.note}>
					{t(
						"Add this line to ~/.ssh/authorized_keys on the device. It lets the phone run only maxd proxy."
					)}
				</Text>
				{line.status === "ok" ? (
					<>
						<Text selectable style={styles.line}>
							{line.data.line}
						</Text>
						<View style={styles.share}>
							<Button
								label={t("Share")}
								small
								onPress={() => void Share.share({ message: line.data.line })}
							/>
						</View>
					</>
				) : (
					<Text style={styles.failed}>{describe(line.error)}</Text>
				)}
			</View>
			<View style={styles.section}>
				<Text style={styles.heading}>{t("Add a device")}</Text>
				<Input
					placeholder={t("Name")}
					accessibilityLabel={t("Device name")}
					autoCapitalize="none"
					value={name}
					onChangeText={setName}
				/>
				<Input
					placeholder="user@host:22"
					accessibilityLabel={t("Route, as user@host:22")}
					autoCapitalize="none"
					autoCorrect={false}
					value={route}
					onChangeText={setRoute}
				/>
				<Input
					placeholder={t("Host key, as ssh-ed25519 AAAA...")}
					accessibilityLabel={t("Host key")}
					autoCapitalize="none"
					autoCorrect={false}
					value={hostKey}
					onChangeText={setHostKey}
				/>
				<Button label={t("Add device")} tone="primary" onPress={add} />
				{note ? <Text style={styles.note}>{note}</Text> : null}
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	value: { maxWidth: "50%", color: color.secondaryForeground, fontSize: size.small },
	inCard: { paddingHorizontal: 14, paddingBottom: 12 },
	byHand: {
		gap: 16,
		paddingHorizontal: 14,
		paddingVertical: 14,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	section: { gap: 8 },
	share: { alignSelf: "flex-start" },
	heading: { color: color.foregroundStrong, fontSize: size.small, fontWeight: "600" },
	note: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText, fontSize: size.meta },
	line: {
		color: color.terminal,
		fontFamily: font.mono,
		fontSize: size.tiny,
		backgroundColor: color.subpanel,
		borderRadius: radius.sm,
		borderColor: color.border,
		borderWidth: StyleSheet.hairlineWidth,
		padding: 10,
	},
	version: {
		color: color.subtleForeground,
		fontSize: size.tiny,
		textAlign: "center",
		marginTop: 4,
	},
});
