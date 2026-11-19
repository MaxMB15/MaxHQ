// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The form that copies a project to another device, as the desktop has it:
// the project, the checkout it comes from, the device it goes to, a new
// container or a folder there, and the files git ignores to bring along.
// Once it starts, it follows the new workspace and the files until the copy
// ends, with the files it couldn't place, which the new workspace's clone
// banner keeps until you dismiss them. The device it goes to must let
// this phone do everything, and so must the one it comes from once any
// file is ticked. The Mac sends its git identity before a copy into a new
// container; the phone has none to send, so the clone commits as the
// device has it set.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";
import { Platform, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import { formatBytes } from "@maxhq/shared/agent-view/format";
import type { DeviceId } from "@maxhq/shared/bindings";
import { copySize } from "@maxhq/shared/copy/defaults";
import type { CopiedProject, IgnoredFile } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { sentence } from "@maxhq/shared/status/format";
import { all } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";
import {
	checkoutOf,
	containerHint,
	type CopyChoices,
	copyLanded,
	firstChoices,
	planCopy,
	projectsToCopy,
	sourceChoices,
	targetChoices,
} from "@maxhq/shared/workspaces/copy";
import {
	allOrNone,
	allTicked,
	COPY_KINDS,
	copyBlocked,
	type CopyFormChoices,
	type CopyRequest,
	type CopyRun,
	copyTitle,
	kindLabel,
	listedFiles,
	type ListingFor,
	listsIgnored,
	loadListing,
	openingChoices,
	runCopy,
	shownListing,
	type StartedCopy,
	tickedFor,
	tickedPaths,
	tickedText,
	type TicksFor,
	transferPercent,
	transferText,
	withTick,
} from "@maxhq/shared/workspaces/copy-form";
import { keepMissed } from "@maxhq/shared/workspaces/missed";
import { cloneOf, isLive } from "@maxhq/shared/workspaces/state";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { choose } from "../../ui/choose";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { CloneBanner } from "../agent/CloneBanner";
import { Buttons } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { closedGates, closedText, copyGates, gatesOpen } from "./copy-gates";
import { CreationSteps } from "./NewWorkspaceScreen";

// Rows are 40 tall; this makes their touch 44.
const ROW_SLOP = { top: 2, bottom: 2 } as const;

/** A bar of how far something got, or a dim full one while that's unknown. */
const Bar = ({ label, percent }: { label: string; percent: number | null }) => (
	<View
		accessible
		accessibilityRole="progressbar"
		accessibilityLabel={label}
		accessibilityValue={percent === null ? undefined : { min: 0, max: 100, now: percent }}
		style={styles.track}
	>
		<View
			style={[
				styles.fill,
				{ width: `${percent ?? 100}%` },
				percent === null && styles.unknown,
			]}
		/>
	</View>
);

/** A file git ignores, with a box to tick it for the copy. */
const FileRow = ({
	file,
	ticked,
	first,
	onTick,
}: {
	file: IgnoredFile;
	ticked: boolean;
	first: boolean;
	onTick: (on: boolean) => void;
}) => (
	<Pressable
		accessibilityRole="checkbox"
		accessibilityLabel={`${file.path}, ${formatBytes(file.size)}`}
		accessibilityState={{ checked: ticked }}
		hitSlop={ROW_SLOP}
		onPress={() => onTick(!ticked)}
		style={({ pressed }) => [styles.file, !first && styles.line, pressed && styles.pressed]}
	>
		<View style={[styles.box, ticked && styles.ticked]}>
			{ticked ? <Text style={styles.tick}>✓</Text> : null}
		</View>
		<Text numberOfLines={1} ellipsizeMode="middle" style={styles.path}>
			{file.path}
		</Text>
		<Text style={styles.size}>{formatBytes(file.size)}</Text>
	</Pressable>
);

/** The copy on its way: the new workspace being made or cloned, then the files. */
const Copying = ({ started }: { started: StartedCopy }) => {
	const { t } = useI18n();

	const { plan, before, files, run } = started;
	const landed = useView((s) => copyLanded(s.view, plan, before)?.id ?? null);
	const cloning = useView((s) => {
		const workspace = landed ? s.view.tables.workspace.get(landed) : undefined;
		return !!workspace && cloneOf(workspace) !== null;
	});
	const where = useView((s) => deviceName(s.view, plan.target.device));
	const transfer = run.phase === "copying" ? run.transfer : null;
	return (
		<>
			{!landed ? (
				<Text accessibilityLiveRegion="polite" style={styles.note}>
					{t("Waiting for {{where}} to start it.", { where: where })}
				</Text>
			) : plan.target.type === "container" ? (
				<CreationSteps id={landed} planned={plan.where} />
			) : cloning ? (
				<CloneBanner workspace={landed} />
			) : (
				<Text style={[styles.text, styles.ready]}>
					{t("Cloned into {{where}}.", { where: plan.where })}
				</Text>
			)}
			{files > 0 ? (
				<View accessibilityLiveRegion="polite" style={styles.status}>
					<Text style={styles.text}>{transferText(transfer, files)}</Text>
					{transfer ? (
						<Bar label={t("Files")} percent={transferPercent(transfer)} />
					) : null}
				</View>
			) : null}
		</>
	);
};

/** What a finished copy placed, and the files that didn't make it. */
const Result = ({ placed, files }: { placed: CopiedProject; files: number }) => {
	const { t } = useI18n();

	if (files === 0) {
		return (
			<Text accessibilityLiveRegion="polite" style={styles.note}>
				{t("The repo is cloned. No ignored files were ticked.")}
			</Text>
		);
	}
	const missed = placed.failed.length;
	return (
		<View accessibilityLiveRegion="polite" style={styles.status}>
			<Text style={styles.text}>
				{t("{{countText}} files copied.", {
					count: placed.written,
					countText: formatNumber(placed.written),
				})}
			</Text>
			{missed > 0 ? (
				<>
					<Text style={[styles.note, styles.failed]}>
						{t("{{countText}} files weren't copied:", {
							count: missed,
							countText: formatNumber(missed),
						})}
					</Text>
					<Card label={t("Files")}>
						{placed.failed.map((f, i) => (
							<View key={f.path} style={[styles.missed, i > 0 && styles.line]}>
								<Text numberOfLines={1} ellipsizeMode="middle" style={styles.path}>
									{f.path}
								</Text>
								<Text style={styles.note}>{sentence(f.reason)}</Text>
							</View>
						))}
					</Card>
				</>
			) : null}
		</View>
	);
};

/** The copy once it started, from the first step to how it ended. */
const Progress = ({
	started,
	onBack,
	close,
}: {
	started: StartedCopy;
	onBack: () => void;
	close: () => void;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const { plan, run } = started;
	return (
		<ScrollView contentContainerStyle={styles.content}>
			<Text accessibilityRole="header" style={styles.heading}>
				{copyTitle(view, plan, run)}
			</Text>
			{run.phase === "copying" ? (
				<Text style={styles.body}>{t("It keeps going if you close this.")}</Text>
			) : (
				<Text style={styles.where}>{plan.where}</Text>
			)}

			{run.phase === "done" ? (
				<Result placed={run.placed} files={started.files} />
			) : run.phase === "failed" ? (
				<Text accessibilityRole="alert" style={[styles.note, styles.failed]}>
					{`${sentence(run.reason)}.`}
				</Text>
			) : (
				<Copying started={started} />
			)}

			<Buttons>
				{run.phase === "failed" ? (
					<Button label={t("Back")} grow={1} onPress={onBack} />
				) : null}
				<Button
					label={run.phase === "copying" ? t("Close") : t("Done")}
					tone={run.phase === "copying" ? "secondary" : "primary"}
					grow={1}
					onPress={close}
				/>
			</Buttons>
		</ScrollView>
	);
};

const Form = ({ request, close }: { request: CopyRequest; close: () => void }) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const live = (id: DeviceId) => isLive(view, id, now);
	const [choices, setChoices] = useState<CopyFormChoices>(() =>
		openingChoices(view, request, live)
	);
	const [listing, setListing] = useState<ListingFor>({
		for: null,
		listing: { state: "loading" },
	});
	const [ticks, setTicks] = useState<TicksFor>({ for: null, ticked: new Set() });
	const [started, setStarted] = useState<StartedCopy | null>(null);

	const from = choices.from ? view.tables.workspace.get(choices.from) : undefined;
	const fromDevice = from?.device ?? null;
	const lists = listsIgnored(view, choices.from);
	const letsTarget = useLetsPhone(choices.device);
	const letsSource = useLetsPhone(fromDevice);

	// The list follows the checkout picked. A device whose maxd predates
	// copies can't list, so only the repo goes.
	useEffect(() => {
		const id = choices.from;
		if (!id || !lists) return;
		let current = true;
		void loadListing(source, id).then((listing) => {
			if (current) setListing({ for: id, listing });
		});
		return () => {
			current = false;
		};
	}, [source, choices.from, lists]);

	if (started) {
		return <Progress started={started} onBack={() => setStarted(null)} close={close} />;
	}

	const chosen = choices.project ? view.tables.project.get(choices.project) : undefined;
	const shown = shownListing(view, choices.from, listing);
	const files = listedFiles(shown);
	const ticked = tickedFor(choices.from, ticks, files);
	const sized = copySize(files, ticked);
	const plan = planCopy(view, choices, live);
	const blocked = copyBlocked(plan, shown, sized);
	const gates = copyGates(choices.device, fromDevice, sized.files);
	const lets = { target: letsTarget, source: letsSource };
	const open = gatesOpen(gates, lets);
	const closed = closedGates(view, gates, lets);

	const set = (change: Partial<CopyChoices>) => setChoices((c) => ({ ...c, ...change }));
	const tick = (path: string, on: boolean) =>
		setTicks({ for: choices.from, ticked: withTick(ticked, path, on) });

	const pickProject = () =>
		choose(
			t("Project"),
			projectsToCopy(view).map((option) => ({
				label:
					option.disabled && option.detail
						? `${option.label}: ${option.detail}`
						: option.label,
				disabled: option.disabled || option.value === choices.project,
				pick: () =>
					setChoices((c) => ({
						...c,
						project: option.value,
						...firstChoices(view, option.value, { device: c.device }, live),
					})),
			}))
		);
	const pickSource = () =>
		choose(
			t("From"),
			(choices.project ? sourceChoices(view, choices.project) : []).map((option) => ({
				label: option.detail ? `${option.label}: ${option.detail}` : option.label,
				disabled: option.value === choices.from,
				pick: () => set({ from: option.value }),
			}))
		);
	const pickTarget = () =>
		choose(
			t("To"),
			targetChoices(view, live).map((option) => ({
				label: option.detail ? `${option.label}: ${option.detail}` : option.label,
				disabled: option.disabled || option.value === choices.device,
				pick: () => set({ device: option.value }),
			}))
		);
	const pickKind = () =>
		choose(
			t("Copy into"),
			COPY_KINDS.map((kind) => ({
				label: kindLabel(kind),
				disabled: kind === choices.kind,
				pick: () => set({ kind }),
			}))
		);

	const start = async () => {
		if (!plan.ok || blocked || !open || !choices.from) return;
		const paths = tickedPaths(files, ticked);
		const before = new Set(all(view, "workspace").map((w) => w.id));
		const begun: Omit<StartedCopy, "run"> = { plan, before, files: paths.length };
		const placed = await runCopy(source, choices.from, plan, paths, (run: CopyRun) =>
			setStarted({ ...begun, run })
		);
		keepMissed(source, services.view.getState().view, plan, before, placed);
	};

	return (
		<ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
			<Text style={styles.body}>
				{t(
					"Clones the project's repo on the device and brings the files you tick, which git doesn't track."
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
				<CardRow
					label={t("From")}
					note={from ? checkoutOf(from) : undefined}
					onPress={choices.project ? pickSource : undefined}
				>
					<Text numberOfLines={1} style={styles.value}>
						{fromDevice ? deviceName(view, fromDevice) : t("None")}
					</Text>
				</CardRow>
				<CardRow label={t("To")} onPress={pickTarget}>
					<Text numberOfLines={1} style={styles.value}>
						{choices.device ? deviceName(view, choices.device) : t("None")}
					</Text>
				</CardRow>
				<CardRow label={t("Copy into")} onPress={pickKind}>
					<Text numberOfLines={1} style={styles.value}>
						{kindLabel(choices.kind)}
					</Text>
				</CardRow>
			</Card>

			{choices.kind === "container" ? (
				<View style={styles.field}>
					<Text style={styles.label}>{t("Container name")}</Text>
					<View style={styles.name}>
						<Text style={styles.prefix}>mhq-</Text>
						<Input
							accessibilityLabel={t("Container name")}
							value={choices.container}
							onChangeText={(container) => set({ container })}
							placeholder={
								choices.project ? containerHint(view, choices.project) : "name"
							}
							autoCapitalize="none"
							autoCorrect={false}
							spellCheck={false}
							autoComplete="off"
							style={styles.input}
						/>
					</View>
				</View>
			) : (
				<>
					<View style={styles.field}>
						<Text style={styles.label}>{t("Parent folder")}</Text>
						<Input
							accessibilityLabel={t("Parent folder")}
							value={choices.parent}
							onChangeText={(parent) => set({ parent })}
							placeholder="/home/you/code"
							autoCapitalize="none"
							autoCorrect={false}
							spellCheck={false}
							autoComplete="off"
							style={styles.mono}
						/>
					</View>
					<View style={styles.field}>
						<Text style={styles.label}>{t("Folder name")}</Text>
						<Input
							accessibilityLabel={t("Folder name")}
							value={choices.folder}
							onChangeText={(folder) => set({ folder })}
							autoCapitalize="none"
							autoCorrect={false}
							spellCheck={false}
							autoComplete="off"
							style={styles.mono}
						/>
					</View>
				</>
			)}

			<View style={styles.field}>
				<View style={styles.filesHead}>
					<Text style={[styles.label, styles.grow]}>{t("Ignored files to copy")}</Text>
					{files.length > 0 ? (
						<>
							<Text style={styles.count}>{tickedText(sized, files.length)}</Text>
							<Button
								label={allTicked(files, sized) ? t("None") : t("All")}
								tone="link"
								small
								onPress={() =>
									setTicks({ for: choices.from, ticked: allOrNone(files, sized) })
								}
							/>
						</>
					) : null}
				</View>
				{shown.state === "failed" ? (
					<Text style={[styles.note, styles.failed]}>{`${sentence(shown.reason)}.`}</Text>
				) : shown.state === "unlisted" ? (
					shown.why ? (
						<Text style={styles.note}>{shown.why}</Text>
					) : null
				) : files.length === 0 ? (
					<Text style={styles.note}>
						{shown.state === "listed"
							? t("git ignores no files here, so only the repo is cloned.")
							: t("Listing the files git ignores.")}
					</Text>
				) : (
					<Card label={t("Ignored files")}>
						{files.map((file, i) => (
							<FileRow
								key={file.path}
								file={file}
								first={i === 0}
								ticked={ticked.has(file.path)}
								onTick={(on) => tick(file.path, on)}
							/>
						))}
					</Card>
				)}
				{shown.state === "listed" && shown.truncated ? (
					<Text style={styles.note}>
						{t("Showing the first {{length}} files git ignores.", {
							length: formatNumber(files.length),
						})}
					</Text>
				) : null}
				{sized.look === "large" ? (
					<Text style={[styles.note, styles.large]}>
						{t("That's {{value1}}, over 100 MB, so the copy takes a while.", {
							value1: formatBytes(sized.bytes),
						})}
					</Text>
				) : null}
				{files.length > 0 ? (
					<Text style={styles.note}>
						{t(
							"The env files start ticked. Folders git ignores whole, like node_modules, are left out."
						)}
					</Text>
				) : null}
			</View>

			{blocked ? (
				<Text accessibilityLiveRegion="polite" style={styles.note}>
					{blocked}
				</Text>
			) : null}

			<View>
				<Buttons>
					<Button label={t("Cancel")} grow={1} onPress={close} />
					<Button
						label={t("Copy")}
						tone="primary"
						grow={1}
						disabled={blocked !== null || !open}
						onPress={() => void start()}
					/>
				</Buttons>
				{open ? null : (
					<>
						<Text style={styles.gate}>
							{t("Let this phone do everything is off for {{devices}}.", {
								devices: closedText(closed),
							})}
						</Text>
						<NeedsSwitch />
					</>
				)}
			</View>
		</ScrollView>
	);
};

export const CopyProjectScreen = ({ navigation, route }: ScreenProps<"CopyProject">) => {
	const { t } = useI18n();

	const close = () => navigation.goBack();
	return (
		<View style={styles.screen}>
			{/* On iOS the modal is a sheet below the status bar; elsewhere it fills the screen. */}
			<Header
				title={t("Copy to another device")}
				sheet={Platform.OS === "ios"}
				back={{ label: t("Close"), onPress: close, close: true }}
			/>
			<Form request={route.params ?? {}} close={close} />
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 12, paddingBottom: 40 },
	heading: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "700" },
	body: { color: color.secondaryForeground, fontSize: size.small, lineHeight: 19 },
	text: { color: color.foreground, fontSize: size.small, lineHeight: 19 },
	note: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	where: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.meta },
	ready: { color: color.readyText },
	large: { color: color.connectingText },
	failed: { color: color.errorText },
	value: { flexShrink: 1, color: color.mutedForeground, fontSize: size.small },
	field: { gap: 6 },
	grow: { flex: 1 },
	label: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "600" },
	name: { flexDirection: "row", alignItems: "center", gap: 6 },
	prefix: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.small },
	input: { flex: 1, fontFamily: font.mono, fontSize: size.small },
	mono: { fontFamily: font.mono, fontSize: size.small },
	filesHead: { flexDirection: "row", alignItems: "center", gap: 8 },
	count: { color: color.mutedForeground, fontSize: size.meta, fontVariant: ["tabular-nums"] },
	file: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		minHeight: 40,
		paddingHorizontal: 14,
	},
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	pressed: { backgroundColor: color.accent },
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
	path: { flex: 1, color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	size: { color: color.mutedForeground, fontSize: size.meta, fontVariant: ["tabular-nums"] },
	missed: { gap: 2, paddingHorizontal: 14, paddingVertical: 10 },
	status: { gap: 8 },
	track: {
		height: 4,
		borderRadius: 2,
		overflow: "hidden",
		backgroundColor: color.track,
	},
	fill: { height: 4, borderRadius: 2, backgroundColor: color.connecting },
	unknown: { opacity: 0.5 },
	gate: { color: color.mutedForeground, fontSize: size.small, marginTop: 6 },
});
