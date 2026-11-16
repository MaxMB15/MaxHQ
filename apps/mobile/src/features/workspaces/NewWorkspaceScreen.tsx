// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The form that makes a container workspace, opened from a device's page:
// the project, the device with Incus, the container's name and whether it
// starts at boot. Once the device takes it, the form follows the workspace
// step by step until it runs or fails. Making one needs the Mac's switch
// for this phone. The Mac sends its git identity before it makes one; the
// phone has none to send, so the clone commits as the device has it set.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Platform, ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId, WorkspaceId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { useNow } from "@maxhq/shared/lib/clock";
import { plural, sentence } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";
import {
	creationSteps,
	madeInstance,
	madeOrFailed,
	type SeenSteps,
	seeStep,
	stepsLeft,
} from "@maxhq/shared/workspaces/creation";
import {
	defaultName,
	egressText,
	guestText,
	hostChoices,
	isLive,
	planWorkspace,
	projectChoices,
	stateLook,
	stateText,
	type WorkspaceChoices,
} from "@maxhq/shared/workspaces/state";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { choose } from "../../ui/choose";
import { ConnectionDot } from "../../ui/Dot";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { Toggle } from "../../ui/Toggle";
import { Buttons } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { LOOK_TEXT } from "./look";

/** A workspace being made, step by step, then its state, guest and egress. */
const CreationSteps = ({ id, planned }: { id: WorkspaceId; planned: string }) => {
	const { t } = useI18n();

	const workspace = useView((s) => s.view.tables.workspace.get(id));
	const now = useNow(5000);
	// The device names the container, so its name wins over the one planned.
	const instance = madeInstance(workspace, planned);
	const managed = workspace?.managed ?? undefined;
	const state = managed?.state;
	// The device names only the step it's on, so the steps already seen are
	// kept here by their place.
	const [seen, setSeen] = useState<SeenSteps>(() => new Map());
	const next = seeStep(seen, state);
	if (next !== seen) setSeen(next);
	const left = stepsLeft(state);

	return (
		<>
			<Card label={t("Steps")}>
				{creationSteps(next, state).map((step, i) => (
					<View
						key={step.at}
						style={[styles.step, i > 0 && styles.line]}
						accessible
						accessibilityLabel={`${formatNumber(step.at + 1)}. ${step.text}: ${step.label}`}
					>
						<ConnectionDot look={step.look} size={8} surface={color.card} decorative />
						<Text style={styles.stepText}>{step.text}</Text>
					</View>
				))}
				{left > 0 ? (
					<Text style={[styles.step, styles.left, styles.line]}>
						{plural(left, "more step")}
					</Text>
				) : null}
			</Card>

			{!workspace || !managed ? (
				<Text accessibilityLiveRegion="polite" style={styles.note}>
					{t("{{instance}} is gone from the device.", { instance: instance })}
				</Text>
			) : (
				<View accessibilityLiveRegion="polite" style={styles.status}>
					<Text style={[styles.state, { color: LOOK_TEXT[stateLook(managed.state)] }]}>
						{managed.state.state === "running"
							? t("{{instance}} is ready.", { instance: instance })
							: `${stateText(managed.state)}.`}
					</Text>
					{managed.state.state === "creating" ? null : (
						<Text style={styles.note}>{guestText(managed.guest, now)}</Text>
					)}
					<Text style={[styles.note, managed.egress.state === "failed" && styles.failed]}>
						{egressText(managed.egress, now)}
					</Text>
				</View>
			)}
		</>
	);
};

/** The workspace being made, step by step, then how it ended. */
const Progress = ({
	id,
	planned,
	close,
}: {
	id: WorkspaceId;
	planned: string;
	close: () => void;
}) => {
	const { t } = useI18n();

	const workspace = useView((s) => s.view.tables.workspace.get(id));
	const ended = madeOrFailed(workspace);
	return (
		<ScrollView contentContainerStyle={styles.content}>
			<Text accessibilityRole="header" style={styles.heading}>
				{t("Making {{value1}}", { value1: madeInstance(workspace, planned) })}
			</Text>
			<Text style={styles.body}>
				{ended
					? t("The device has finished.")
					: t("It keeps going on the device if you close this.")}
			</Text>
			<CreationSteps id={id} planned={planned} />
			<Buttons>
				<Button
					label={ended ? t("Done") : t("Close")}
					tone={ended ? "primary" : "secondary"}
					grow={1}
					onPress={close}
				/>
			</Buttons>
		</ScrollView>
	);
};

const Form = ({
	device,
	close,
	made,
}: {
	device: DeviceId;
	close: () => void;
	made: (id: WorkspaceId, instance: string) => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const live = (id: DeviceId) => isLive(view, id, now);
	const [choices, setChoices] = useState<WorkspaceChoices>({
		project: null,
		device,
		name: "",
		startAtBoot: true,
	});
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);
	const lets = useLetsPhone(choices.device);

	const plan = planWorkspace(view, choices, live);
	const chosen = choices.project ? view.tables.project.get(choices.project) : undefined;
	const set = (change: Partial<WorkspaceChoices>) => {
		setFailure(null);
		setChoices((c) => ({ ...c, ...change }));
	};

	const pickProject = () =>
		choose(
			t("Project"),
			projectChoices(view).map((option) => ({
				label: option.label,
				disabled: option.disabled || option.value === choices.project,
				pick: () => set({ project: option.value }),
			}))
		);
	const pickDevice = () =>
		choose(
			t("Device"),
			hostChoices(view, live).map((option) => ({
				label: option.detail ? `${option.label}: ${option.detail}` : option.label,
				disabled: option.disabled || option.value === choices.device,
				pick: () => set({ device: option.value }),
			}))
		);

	const create = async () => {
		if (!plan.ok || busy) return;
		setBusy(true);
		setFailure(null);
		try {
			const result = await source.createWorkspace(plan.spec);
			if (result.status === "ok" && result.data.status === "created") {
				made(result.data.workspace, plan.instance);
			} else {
				setFailure(`${sentence(failureOf(result) ?? t("The device didn't make it"))}.`);
			}
		} catch (error) {
			console.warn("Couldn't create the workspace", error);
			setFailure(t("Couldn't reach the device. Try again."));
		} finally {
			setBusy(false);
		}
	};

	return (
		<ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
			<Text style={styles.body}>
				{t(
					"A container on a device with Incus, with the project's repo cloned in it. MaxHQ checks what the container can reach before any agent starts there."
				)}
			</Text>

			<Card>
				<CardRow
					first
					label={t("Project")}
					note={chosen?.repo ?? undefined}
					onPress={pickProject}
				>
					<Text numberOfLines={1} style={styles.value}>
						{chosen?.name ?? t("None")}
					</Text>
				</CardRow>
				<CardRow label={t("Device")} onPress={pickDevice}>
					<Text numberOfLines={1} style={styles.value}>
						{choices.device ? deviceName(view, choices.device) : t("None")}
					</Text>
				</CardRow>
				<CardRow
					label={t("Start at boot")}
					note={t("Starts the container whenever the device boots.")}
				>
					<Toggle
						label={t("Start at boot")}
						value={choices.startAtBoot}
						onChange={(on) => set({ startAtBoot: on })}
					/>
				</CardRow>
			</Card>

			<View style={styles.field}>
				<Text style={styles.label}>{t("Container name")}</Text>
				<View style={styles.name}>
					<Text style={styles.prefix}>mhq-</Text>
					<Input
						accessibilityLabel={t("Container name")}
						value={choices.name}
						onChangeText={(name) => set({ name })}
						placeholder={chosen ? defaultName(chosen.name) : "name"}
						autoCapitalize="none"
						autoCorrect={false}
						spellCheck={false}
						autoComplete="off"
						returnKeyType="done"
						onSubmitEditing={() => void create()}
						style={styles.input}
					/>
				</View>
			</View>

			{failure || !plan.ok ? (
				<Text
					accessibilityLiveRegion="polite"
					style={[styles.note, failure !== null && styles.failed]}
				>
					{failure ?? (plan.ok ? null : plan.reason)}
				</Text>
			) : null}

			<Buttons>
				<Button label={t("Cancel")} grow={1} onPress={close} />
				<Button
					label={busy ? t("Creating") : t("Create")}
					tone="primary"
					grow={1}
					disabled={!plan.ok || busy || !lets}
					onPress={() => void create()}
				/>
			</Buttons>
			{lets ? null : <NeedsSwitch />}
		</ScrollView>
	);
};

export const NewWorkspaceScreen = ({ navigation, route }: ScreenProps<"NewWorkspace">) => {
	const { t } = useI18n();

	const [made, setMade] = useState<{ id: WorkspaceId; instance: string } | null>(null);
	const close = () => navigation.goBack();
	return (
		<View style={styles.screen}>
			{/* On iOS the modal is a sheet below the status bar; elsewhere it fills the screen. */}
			<Header
				title={t("New workspace")}
				sheet={Platform.OS === "ios"}
				back={{ label: t("Close"), onPress: close, close: true }}
			/>
			{made ? (
				<Progress id={made.id} planned={made.instance} close={close} />
			) : (
				<Form
					device={route.params.device}
					close={close}
					made={(id, instance) => setMade({ id, instance })}
				/>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 12, paddingBottom: 40 },
	heading: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "700" },
	body: { color: color.secondaryForeground, fontSize: size.small, lineHeight: 19 },
	note: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	failed: { color: color.errorText },
	value: { flexShrink: 1, color: color.mutedForeground, fontSize: size.small },
	field: { gap: 6 },
	label: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "600" },
	name: { flexDirection: "row", alignItems: "center", gap: 6 },
	prefix: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.small },
	input: { flex: 1, fontFamily: font.mono, fontSize: size.small },
	step: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 14,
		paddingVertical: 10,
	},
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	stepText: { flex: 1, color: color.foreground, fontSize: size.small },
	left: { color: color.mutedForeground, fontSize: size.meta, paddingLeft: 32 },
	status: { gap: 4 },
	state: { fontSize: size.small },
});
