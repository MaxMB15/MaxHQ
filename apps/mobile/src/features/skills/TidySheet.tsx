// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The consent step for tidying a device's global skills, as the desktop's
// dialog: the device's plan in full, which folders move into the library,
// which repos join as sources, which links appear and which copies go to
// the backup, a pick for each skill whose copies differ, then a switch for
// the device and Apply. Apply sends the plan's hash and the picks, so the
// device does what you saw or nothing. Reading the plan is open to every
// phone; Apply takes the Mac's switch for the device.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Modal, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId, GlobalsConflict, GlobalsJoin } from "@maxhq/shared/bindings";
import { readOf } from "@maxhq/shared/pulls/read";
import { globalsPlanEmpty } from "@maxhq/shared/skills/project";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, font, radius, size, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";
import { Check } from "../../ui/glyphs";
import { Header } from "../../ui/Header";
import { Muted, NoteLine, useAction } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { useRead } from "../pulls/read";
import { Tick } from "../tasks/fields";

const Heading = ({ children }: { children: string }) => (
	<Text accessibilityRole="header" style={styles.heading}>
		{children}
	</Text>
);

const List = ({ title, paths }: { title: string; paths: readonly string[] }) =>
	paths.length > 0 ? (
		<View accessibilityLabel={title} style={styles.part}>
			<Heading>{title}</Heading>
			<View style={styles.box}>
				{paths.map((path) => (
					<Text key={path} style={styles.mono} numberOfLines={1} ellipsizeMode="head">
						{path}
					</Text>
				))}
			</View>
		</View>
	) : null;

const Joins = ({ joins }: { joins: readonly GlobalsJoin[] }) => {
	const { t } = useI18n();
	return joins.length > 0 ? (
		<View style={styles.part}>
			<Heading>{t("Join as sources")}</Heading>
			<View style={styles.box}>
				{joins.map((join) => (
					<View key={join.url} style={styles.join}>
						<Text style={styles.text}>
							{join.name}{" "}
							<Text style={styles.muted}>{t("from {{url}}", { url: join.url })}</Text>
						</Text>
						{join.paths.map((path) => (
							<Text
								key={path}
								style={[styles.mono, styles.muted]}
								numberOfLines={1}
								ellipsizeMode="head"
							>
								{path}
							</Text>
						))}
					</View>
				))}
			</View>
		</View>
	) : null;
};

const Conflicts = ({
	conflicts,
	picks,
	onPick,
}: {
	conflicts: readonly GlobalsConflict[];
	picks: Readonly<Record<string, string>>;
	onPick: (name: string, path: string) => void;
}) => {
	const { t } = useI18n();
	return conflicts.length > 0 ? (
		<View style={styles.part}>
			<Heading>{t("Pick one copy")}</Heading>
			<Muted>
				{t(
					"These copies differ. The one you pick moves into the library and the rest go to the backup. Without a pick they stay where they are."
				)}
			</Muted>
			<View style={styles.box}>
				{conflicts.map((conflict) => (
					<View key={conflict.name} accessibilityRole="radiogroup" style={styles.join}>
						<Text style={styles.text}>{conflict.name}</Text>
						{conflict.paths.map((path) => {
							const on = picks[conflict.name] === path;
							return (
								<Pressable
									key={path}
									accessibilityRole="radio"
									accessibilityState={{ checked: on }}
									accessibilityLabel={path}
									onPress={() => onPick(conflict.name, path)}
									style={({ pressed }) => [
										styles.pick,
										pressed && styles.pressed,
									]}
								>
									<View style={[styles.radio, on && styles.radioOn]}>
										{on ? (
											<Check
												size={11}
												strokeWidth={3}
												color={color.background}
											/>
										) : null}
									</View>
									<Text style={[styles.mono, styles.grow]} numberOfLines={2}>
										{path}
									</Text>
								</Pressable>
							);
						})}
					</View>
				))}
			</View>
		</View>
	) : null;
};

const Body = ({ device, onDone }: { device: DeviceId; onDone: () => void }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const name = useView((s) => deviceName(s.view, device));
	const lets = useLetsPhone(device);
	const [ticked, setTicked] = useState(false);
	const [picks, setPicks] = useState<Record<string, string>>({});
	const { busy, note, run } = useAction();
	const plan = useRead(
		`tidy\n${device}`,
		async () => readOf(await source.planGlobalSkills(device), "skills_globals_plan").plan
	);
	const apply = async (hash: string) => {
		const done = await run(
			t("Tidying"),
			() => source.applyGlobalSkills(device, hash, Object.values(picks)),
			t("Tidied")
		);
		if (done) onDone();
	};
	const data = plan.data;
	return (
		<ScrollView contentContainerStyle={styles.content}>
			<Text style={styles.intro}>
				{t(
					"Your own global skills move into the library, so you pick them per project. The skills every agent gets stay global, as links MaxHQ keeps."
				)}
			</Text>
			{!data ? (
				plan.error ? (
					<Text accessibilityRole="alert" style={styles.error}>
						{plan.error}
					</Text>
				) : (
					<Muted>{t("Reading the global folders")}</Muted>
				)
			) : globalsPlanEmpty(data) ? (
				<Muted>{t("There's nothing to tidy on {{name}}.", { name: name })}</Muted>
			) : (
				<>
					<List title={t("Move into the library")} paths={data.moves} />
					<Joins joins={data.joins ?? []} />
					<Conflicts
						conflicts={data.conflicts ?? []}
						picks={picks}
						onPick={(skill, path) => setPicks((now) => ({ ...now, [skill]: path }))}
					/>
					<List title={t("Links that appear")} paths={data.links} />
					<List title={t("Copies that go to the backup")} paths={data.backups} />
					<View style={styles.part}>
						<Muted>
							{t(
								"Nothing is deleted. Moved folders go to a dated backup folder, and Undo puts them back."
							)}
						</Muted>
						<Muted>
							{t(
								"MaxHQ writes only skill folders and links. It never touches settings.json, settings.local.json, config.toml or skillOverrides."
							)}
						</Muted>
						<Muted>
							{t(
								"Hiding a plugin or system skill stays your job, in that CLI's own settings."
							)}
						</Muted>
					</View>
					<Tick
						label={t("Tidy {{name}}", { name: name })}
						value={ticked}
						onChange={setTicked}
					/>
					<Button
						label={t("Apply")}
						tone="primary"
						disabled={!ticked || busy || !lets}
						onPress={() => void apply(data.hash)}
					/>
					<NoteLine note={note} />
					{!lets ? <NeedsSwitch /> : null}
				</>
			)}
		</ScrollView>
	);
};

/** The tidy sheet for one device, or shut for none. `onDone` runs once the device tidied. */
export const TidySheet = ({
	device,
	onClose,
	onDone,
}: {
	device: DeviceId | null;
	onClose: () => void;
	onDone: () => void;
}) => {
	const { t } = useI18n();
	const name = useView((s) => (device ? deviceName(s.view, device) : ""));
	return (
		<Modal
			visible={device !== null}
			animationType="slide"
			presentationStyle="pageSheet"
			onRequestClose={onClose}
		>
			<View style={styles.screen}>
				<Header
					title={t("Tidy global skills on {{name}}", { name: name })}
					sheet
					back={{ label: t("Cancel"), onPress: onClose, close: true }}
				/>
				{device ? <Body key={device} device={device} onDone={onDone} /> : null}
			</View>
		</Modal>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	intro: { color: color.secondaryForeground, fontSize: size.small },
	error: { color: color.errorText, fontSize: size.small },
	part: { gap: 6 },
	heading: { color: color.secondaryForeground, fontSize: size.tiny, fontWeight: "600" },
	box: {
		gap: 4,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
		paddingHorizontal: 10,
		paddingVertical: 8,
	},
	join: { gap: 2 },
	text: { color: color.foreground, fontSize: size.meta },
	muted: { color: color.mutedForeground },
	mono: { color: color.foreground, fontFamily: font.mono, fontSize: size.tiny },
	grow: { flex: 1 },
	pick: { minHeight: TOUCH, flexDirection: "row", alignItems: "center", gap: 10 },
	pressed: { backgroundColor: color.accent },
	radio: {
		width: 18,
		height: 18,
		borderRadius: 9,
		borderWidth: 1.5,
		borderColor: color.input,
		alignItems: "center",
		justifyContent: "center",
	},
	radioOn: { backgroundColor: color.primary, borderColor: color.primary },
});
