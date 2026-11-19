// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Settings, as a short first page of cards whose rows open a page each:
// this phone's name and language, the Macs it pairs with, About, then the
// app's pages (the agent list, the composer, notifications, the dispatcher,
// agent tools and the skill library), the logins, each project's settings, and the devices
// with each one's settings and, by hand, the line that lets the phone into
// a device and a form to add one. Opened from the first screen's "Add a
// device by hand", the form starts open and in view.

import { type LanguageChoice, useI18n } from "@maxhq/shared/i18n";

import { type ComponentRef, type Ref, useRef, useState } from "react";
import { ScrollView, Share, StyleSheet, Text, View } from "react-native";

import { describe } from "@maxhq/shared/data/request-source";
import { useNow } from "@maxhq/shared/lib/clock";
import { LOGIN_HINT, LOGIN_PAGES, LOGIN_STEPS } from "@maxhq/shared/settings/login-steps";
import { deviceReach } from "@maxhq/shared/status/reach";
import { all, type ViewState } from "@maxhq/shared/view/replica";

import type { PhoneCalls } from "../../data/native-source";
import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose } from "../../ui/choose";
import { Card, CardRow } from "../../ui/Card";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { reachSummary } from "../devices/summary";
import { phoneName, renameNote } from "../macs/macs";
import { PairedMacsCard } from "../macs/PairedMacsCard";
import { useMacs } from "../macs/useMacs";
import { type SettingsPage, settingsPageTitle } from "../settings/SettingsPageScreen";
import { AboutSection } from "../updates/AboutSection";

type Navigation = ScreenProps<"Settings">["navigation"];

const APP_PAGES: readonly SettingsPage[] = [
	"agent-list",
	"composer",
	"notifications",
	"dispatcher",
	"agent-tools",
];

/** The rows that open the app's pages. */
const AppCard = ({ navigation }: { navigation: Navigation }) => {
	const { t } = useI18n();
	return (
		<Card title={t("App")}>
			{APP_PAGES.map((page, i) => (
				<CardRow
					key={page}
					first={i === 0}
					label={settingsPageTitle(t, page)}
					onPress={() => navigation.navigate("SettingsPage", { page })}
				/>
			))}
			<CardRow
				label={t("Skills")}
				note={t("The skills projects pick from, and the ones every agent gets")}
				onPress={() => navigation.navigate("SkillLibrary")}
			/>
		</Card>
	);
};

/** The rows that open each login's page. */
const LoginsCard = ({ navigation }: { navigation: Navigation }) => {
	const { t } = useI18n();
	return (
		<Card title={t("Logins")}>
			{LOGIN_PAGES.map((login, i) => (
				<CardRow
					key={login}
					first={i === 0}
					label={LOGIN_STEPS[login].title}
					note={LOGIN_HINT[login]}
					onPress={() => navigation.navigate("SignIn", { login })}
				/>
			))}
		</Card>
	);
};

/** A row for each project, opening its settings. */
const ProjectsCard = ({ navigation, view }: { navigation: Navigation; view: ViewState }) => {
	const { t } = useI18n();
	const projects = all(view, "project").sort((a, b) => a.name.localeCompare(b.name));
	return (
		<Card title={t("Projects")}>
			{projects.length === 0 ? (
				<CardRow first label={t("No projects yet")} />
			) : (
				projects.map((project, i) => (
					<CardRow
						key={project.id}
						first={i === 0}
						label={project.name}
						onPress={() =>
							navigation.navigate("ProjectSettings", { project: project.id })
						}
					/>
				))
			)}
		</Card>
	);
};

export const PhoneScreen = ({ navigation, route }: ScreenProps<"Settings">) => {
	const { t } = useI18n();

	const { phone } = useServices();
	const [key] = useState(() => phone?.phoneKey());
	const id = key?.status === "ok" ? key.data.phone : null;
	const named = useView((s) => (id ? phoneName(s.view, id) : undefined));
	const { macs } = useMacs();
	const [byHand, setByHand] = useState(route.params?.byHand === true);
	const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
	const form = useRef<ComponentRef<typeof View>>(null);
	const jump = useRef(byHand);
	const view = useView((s) => s.view);
	const now = useNow(10_000);
	const devices = deviceReach(view, now);
	// Each paired device once, by name, for its settings.
	const links = all(view, "device_link")
		.filter((link) => link.device !== null && !link.claim)
		.sort((a, b) => a.name.localeCompare(b.name));
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
					<Card title={t("This phone")}>
						<LanguageRow first />
					</Card>
					<AppCard navigation={navigation} />
				</ScrollView>
			</View>
		);
	}

	// Scrolls to the form once, when the screen opened with it.
	const toForm = () => {
		const content = scroll.current?.getInnerViewRef();
		if (!jump.current || !content) return;
		jump.current = false;
		form.current?.measureLayout(content, (_x, y) => scroll.current?.scrollTo({ y }));
	};

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
					<CardRow label={t("Name")} note={named ? renameNote(macs) : undefined}>
						<Text style={styles.value} numberOfLines={1}>
							{named ?? t("Not paired yet")}
						</Text>
					</CardRow>
					<LanguageRow />
				</Card>
				<PairedMacsCard phone={phone} onPair={() => navigation.navigate("Scan")} />
				<AboutSection />
				<AppCard navigation={navigation} />
				<LoginsCard navigation={navigation} />
				<ProjectsCard navigation={navigation} view={view} />
				{links.length > 0 ? (
					<Card title={t("Device settings")}>
						{links.map((link, i) => {
							const device = link.device;
							return device ? (
								<CardRow
									key={link.id}
									first={i === 0}
									label={link.name}
									onPress={() =>
										navigation.navigate("DeviceSettings", { device })
									}
								/>
							) : null;
						})}
					</Card>
				) : null}
				<Card title={t("Devices")}>
					<CardRow
						label={t("Devices")}
						note={devices.length > 0 ? reachSummary(devices) : undefined}
						onPress={() => navigation.navigate("Devices")}
					/>
					<CardRow
						label={t("Add a device by hand")}
						note={t("For a device without a Mac nearby")}
						onPress={() => setByHand((open) => !open)}
					/>
					{byHand ? (
						<ByHand ref={form} phone={phone} line={key} onLayout={toForm} />
					) : null}
				</Card>
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

const LanguageRow = ({ first }: { first?: boolean }) => {
	const { t, choice, setLanguageChoice } = useI18n();
	const label = (value: LanguageChoice) =>
		value === "system" ? t("System") : t(LANGUAGE_NAMES[value]);
	return (
		<CardRow
			first={first}
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
});
