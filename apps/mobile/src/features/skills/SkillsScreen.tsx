// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's skills, as the desktop's Skills tab: banners for checkouts
// whose copies changed or that links lead out of, the proposals waiting,
// then on each device that keeps the project's skills its own set with
// Update, Pin and Remove, skills to add from the library or another
// project, its devflow pack and Apply, and what each CLI sees there as the
// device's scan found it. Every change goes to the device at once and
// takes the Mac's switch for that device; looking and scanning don't.
// Opened from the project menu and the project's settings.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { FlatList, Modal, ScrollView, StyleSheet, Text, View } from "react-native";

import type {
	AgentPlace,
	CheckoutSkills,
	DeviceId,
	ProjectId,
	SkillLibrary,
	SkillLock,
	SkillProposalId,
} from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	type Budget,
	cliLabel,
	type CliSkills,
	markLabel,
	originLabel,
	skillsByCli,
} from "@maxhq/shared/skills/effective";
import {
	addFromProject,
	cantLock,
	driftOf,
	type LibrarySkill,
	lockedFrom,
	outsideOf,
	searchLibrary,
	skillBadges,
	skillLibrary,
	skillProposals,
} from "@maxhq/shared/skills/library";
import {
	PACK_LEFT_FILES,
	packHasUpdate,
	packSources,
	packStateText,
	withPack,
} from "@maxhq/shared/skills/pack";
import {
	budgetsOf,
	EMPTY_LOCK,
	hasUpdates,
	hideHint,
	lockedWhere,
	needsApply,
	otherLocks,
	pinSkill,
	projectSkillsOn,
	proposalFrom,
	pushedOut,
	scanPlaceOf,
	skillDevices,
	updatedLock,
	updateOf,
	withoutSkill,
} from "@maxhq/shared/skills/project";
import { rescanSkills, useSkillScan, useSteadyPlace } from "@maxhq/shared/skills/scan";
import { formatAgo, listInWords } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { choose, confirm } from "../../ui/choose";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { CardBody, Muted, type Note, NoteLine, useAction } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Tabs } from "../pulls/Tabs";
import { Picker } from "../tasks/Picker";
import { Badge, GatedToggle } from "../test-hosts/parts";
import { Banner, BudgetMeter, ProposalRow, SkillBadges, SkillName } from "./look";
import { shownDevice } from "./skills";

/** Saves the project's set on the device, saying `doing` while it waits and `done` after. */
type Save = (next: SkillLock, doing: string, done: string) => void;

const OutsideBanner = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const found = outsideOf(view, project);
	if (found.length === 0) return null;
	return (
		<Banner label={t("Skills that can't land")}>
			{found.map(({ device, checkout }) => (
				<Text key={`${device}:${checkout.path}`} style={styles.bannerText}>
					{t(
						"Skills can't land through {{value1}} in {{path}} on {{value3}}: a link there leads out of the checkout.",
						{
							value1: listInWords(checkout.outside ?? []),
							path: checkout.path,
							value3: deviceName(view, device),
						}
					)}
				</Text>
			))}
		</Banner>
	);
};

/** One checkout whose copies changed, with Restore, which takes the device's switch. */
const DriftRow = ({
	project,
	device,
	checkout,
}: {
	project: ProjectId;
	device: DeviceId;
	checkout: CheckoutSkills;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const name = useView((s) => deviceName(s.view, device));
	const lets = useLetsPhone(device);
	const { busy, note, run } = useAction();
	return (
		<View style={styles.drift}>
			<Text style={styles.bannerText}>
				{t("{{value1}} changed in {{path}} on {{value3}} since it landed.", {
					value1: listInWords(checkout.drift),
					path: checkout.path,
					value3: name,
				})}
			</Text>
			<View style={styles.start}>
				<Button
					label={t("Restore")}
					small
					disabled={busy || !lets}
					onPress={() =>
						void run(
							t("Restoring"),
							() => source.applySkills(device, project),
							t("Restored")
						)
					}
				/>
			</View>
			<NoteLine note={note} />
			{!lets ? <NeedsSwitch /> : null}
		</View>
	);
};

const DriftBanner = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const drift = driftOf(view, project);
	if (drift.length === 0) return null;
	return (
		<Banner label={t("Changed skills")}>
			{drift.map(({ device, checkout }) => (
				<DriftRow
					key={`${device}:${checkout.path}`}
					project={project}
					device={device}
					checkout={checkout}
				/>
			))}
		</Banner>
	);
};

const Proposals = ({
	project,
	onOpen,
}: {
	project: ProjectId;
	onOpen: (id: SkillProposalId) => void;
}) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const proposals = skillProposals(view, project);
	if (proposals.length === 0) return null;
	return (
		<Card title={t("Proposals")} note={t("Changes agents want in your skills")}>
			{proposals.map((proposal) => (
				<ProposalRow
					key={proposal.id}
					proposal={proposal}
					line={t("From {{value1}}, {{value2}}", {
						value1: proposalFrom(view, proposal),
						value2: formatAgo(proposal.created_at, now),
					})}
					onPress={() => onOpen(proposal.id)}
				/>
			))}
		</Card>
	);
};

/** One skill of the library to add, or why it can't be. */
const LibraryRow = ({
	item,
	why,
	off,
	onAdd,
}: {
	item: LibrarySkill;
	why: string | null;
	off: boolean;
	onAdd: () => void;
}) => {
	const { t } = useI18n();
	return (
		<View style={styles.pickRow}>
			<SkillName name={item.skill.name} line={why ?? item.skill.description}>
				<SkillBadges badges={skillBadges(item.skill, item.source)} />
			</SkillName>
			<Button
				label={t("Add")}
				hint={t("Add {{name}}", { name: item.skill.name })}
				small
				disabled={off || why !== null}
				onPress={onAdd}
			/>
		</View>
	);
};

/** The library to pick from, in a sheet that stays open while you add several. */
const LibrarySheet = ({
	open,
	library,
	lock,
	busy,
	lets,
	note,
	onAdd,
	onClose,
}: {
	open: boolean;
	library: SkillLibrary;
	lock: SkillLock;
	busy: boolean;
	lets: boolean;
	note: Note | null;
	onAdd: (next: SkillLock, name: string) => void;
	onClose: () => void;
}) => {
	const { t } = useI18n();
	const [query, setQuery] = useState("");
	const found = searchLibrary(library, query);
	return (
		<Modal
			visible={open}
			animationType="slide"
			presentationStyle="pageSheet"
			onRequestClose={onClose}
		>
			<View style={styles.screen}>
				<Header
					title={t("Library")}
					sheet
					back={{ label: t("Close"), onPress: onClose, close: true }}
				/>
				<View style={styles.search}>
					<Input
						accessibilityLabel={t("Search the library")}
						placeholder={t("Search skills")}
						value={query}
						onChangeText={setQuery}
						autoCapitalize="none"
						autoCorrect={false}
					/>
					<NoteLine note={note} />
					{!lets ? <NeedsSwitch /> : null}
				</View>
				<FlatList
					data={found}
					keyExtractor={(item) => `${item.source.id}:${item.skill.name}`}
					keyboardShouldPersistTaps="handled"
					contentContainerStyle={styles.list}
					ListEmptyComponent={<Muted>{t("No skills match")}</Muted>}
					renderItem={({ item }) => (
						<LibraryRow
							item={item}
							why={cantLock(library, lock, item.skill.name)}
							off={busy || !lets}
							onAdd={() =>
								onAdd(
									{ ...lock, skills: [...lock.skills, lockedFrom(item)] },
									item.skill.name
								)
							}
						/>
					)}
				/>
			</View>
		</Modal>
	);
};

/** One skill in the project's set, with Update, Pin and Remove. */
const LockedRow = ({
	library,
	lock,
	name,
	off,
	save,
}: {
	library: SkillLibrary;
	lock: SkillLock;
	name: string;
	off: boolean;
	save: Save;
}) => {
	const { t } = useI18n();
	const locked = lock.skills.find((s) => s.name === name)!;
	const update = updateOf(library, locked);
	return (
		<CardBody>
			<SkillName name={locked.name} line={lockedWhere(library, locked)}>
				{locked.pinned ? <Badge label={t("Pinned")} tone="info" /> : null}
			</SkillName>
			<View style={styles.row}>
				{update ? (
					<Button
						label={t("Update")}
						hint={t("Update {{name}}", { name: locked.name })}
						small
						disabled={off}
						onPress={() =>
							save(
								{
									...lock,
									skills: lock.skills.map((s) =>
										s.name === locked.name ? update : s
									),
								},
								t("Updating"),
								t("Updated {{name}}", { name: locked.name })
							)
						}
					/>
				) : null}
				<Button
					label={locked.pinned ? t("Unpin") : t("Pin")}
					hint={locked.name}
					small
					disabled={off}
					onPress={() =>
						save(pinSkill(lock, locked.name, !locked.pinned), t("Saving"), t("Saved"))
					}
				/>
				<Button
					label={t("Remove")}
					tone="danger"
					hint={t("Remove {{name}}", { name: locked.name })}
					small
					disabled={off}
					onPress={() =>
						save(
							withoutSkill(lock, locked.name),
							t("Removing"),
							t("Removed {{name}}", { name: locked.name })
						)
					}
				/>
			</View>
		</CardBody>
	);
};

/** The devflow pack: the source the checkouts install whole, and where each copy stands. */
const PackPart = ({
	library,
	lock,
	checkouts,
	off,
	save,
	onRetry,
}: {
	library: SkillLibrary;
	lock: SkillLock;
	checkouts: readonly CheckoutSkills[];
	off: boolean;
	save: Save;
	onRetry: () => void;
}) => {
	const { t } = useI18n();
	const sources = packSources(library);
	const shown = checkouts.filter((checkout) => checkout.pack);
	if (sources.length === 0 && !lock.pack && shown.length === 0) return null;
	const pick = (value: string) => {
		if (!value && lock.pack) {
			confirm(
				{
					title: t("Remove the devflow pack"),
					detail: `${t(
						"This takes the pack, its links and its exclude lines out of each checkout. These stay, since you may have changed them. Delete them by hand if you don't want them:"
					)}\n\n${PACK_LEFT_FILES.join("\n")}`,
				},
				t("Remove"),
				() => save(withPack(library, lock, null), t("Removing"), t("Removed"))
			);
		} else {
			save(withPack(library, lock, value || null), t("Saving"), t("Saved"));
		}
	};
	return (
		<CardBody>
			<Picker
				label={t("Devflow pack")}
				value={lock.pack?.source ?? ""}
				options={[
					{ value: "", label: t("None") },
					...sources.map((source) => ({ value: source.id, label: source.name })),
				]}
				onChange={pick}
				disabled={off}
			/>
			{packHasUpdate(library, lock) ? (
				<View style={styles.start}>
					<Button
						label={t("Update the pack")}
						small
						disabled={off}
						onPress={() =>
							save(
								withPack(library, lock, lock.pack!.source),
								t("Updating"),
								t("Updated")
							)
						}
					/>
				</View>
			) : null}
			{shown.map((checkout) => (
				<View key={checkout.path} style={styles.pack}>
					<Text style={styles.path} numberOfLines={1} ellipsizeMode="head">
						{checkout.path}
					</Text>
					<Text style={[styles.state, checkout.pack?.state === "failed" && styles.warn]}>
						{packStateText(checkout)}
					</Text>
					{checkout.pack?.state === "failed" ? (
						<>
							<Text selectable style={styles.log}>
								{checkout.pack.lines.join("\n")}
							</Text>
							<View style={styles.start}>
								<Button
									label={t("Retry")}
									hint={t("Retry the pack in {{path}}", { path: checkout.path })}
									small
									disabled={off}
									onPress={onRetry}
								/>
							</View>
						</>
					) : null}
				</View>
			))}
		</CardBody>
	);
};

const ThisProject = ({
	project,
	device,
	library,
	place,
}: {
	project: ProjectId;
	device: DeviceId;
	library: SkillLibrary;
	place: AgentPlace | null;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const lets = useLetsPhone(device);
	const { busy, note, run } = useAction();
	const [adding, setAdding] = useState(false);
	const skills = projectSkillsOn(view, project, device);
	const lock = skills?.lock ?? EMPTY_LOCK;
	const scan = useSkillScan(source, place ? device : null, place);
	const pushed = scan?.skills ? pushedOut(view, project, library, lock, scan.skills) : [];
	const others = otherLocks(view, project, device);
	const off = busy || !lets;

	const save: Save = (next, doing, done) =>
		void run(doing, () => source.setProjectSkills(device, project, next), done);
	const apply = async () => {
		const applied = await run(
			t("Applying"),
			() => source.applySkills(device, project),
			t("Applied")
		);
		if (applied && place) void rescanSkills(source, device, place);
	};
	const fromProject = () =>
		choose(
			t("Add from project"),
			others.map((p) => ({
				label: view.tables.project.get(p.project)?.name ?? p.project,
				pick: () => save(addFromProject(library, lock, p.lock), t("Adding"), t("Added")),
			}))
		);

	return (
		<Card title={t("This project")}>
			{lock.skills.length === 0 ? (
				<CardBody>
					<Muted>{t("No skills yet. Add some from the library.")}</Muted>
				</CardBody>
			) : (
				lock.skills.map((locked) => (
					<LockedRow
						key={locked.name}
						library={library}
						lock={lock}
						name={locked.name}
						off={off}
						save={save}
					/>
				))
			)}
			<CardBody>
				<View style={styles.row}>
					<Button
						label={t("Add from the library")}
						small
						onPress={() => setAdding(true)}
					/>
					{others.length > 0 ? (
						<Button
							label={t("Add from project")}
							small
							disabled={off}
							onPress={fromProject}
						/>
					) : null}
					{hasUpdates(library, lock) ? (
						<Button
							label={t("Update all")}
							small
							disabled={off}
							onPress={() =>
								save(updatedLock(library, lock), t("Updating"), t("Updated"))
							}
						/>
					) : null}
				</View>
			</CardBody>
			<PackPart
				library={library}
				lock={lock}
				checkouts={skills?.checkouts ?? []}
				off={off}
				save={save}
				onRetry={() => void apply()}
			/>
			<CardRow
				label={t("Commit the skills")}
				note={t("Commit the skills, so everyone who clones the repo gets them")}
			>
				<GatedToggle
					label={t("Commit the skills")}
					value={lock.committed}
					disabled={off}
					onChange={(committed) => save({ ...lock, committed }, t("Saving"), t("Saved"))}
				/>
			</CardRow>
			<CardBody>
				{pushed.length > 0 ? (
					<Text accessibilityRole="alert" style={[styles.state, styles.warn]}>
						{t(
							"Applying pushes {{value1}} past its skill listing, so it drops descriptions.",
							{ value1: listInWords(pushed.map(cliLabel)) }
						)}
					</Text>
				) : null}
				{needsApply(skills) ? <Muted>{t("Not in the checkouts yet")}</Muted> : null}
				<Button
					label={t("Apply")}
					tone="primary"
					disabled={off || !skills}
					onPress={() => void apply()}
				/>
				<NoteLine note={note} />
				{!lets ? <NeedsSwitch /> : null}
			</CardBody>
			<LibrarySheet
				open={adding}
				library={library}
				lock={lock}
				busy={busy}
				lets={lets}
				note={note}
				onAdd={(next, name) => save(next, t("Adding"), t("Added {{name}}", { name: name }))}
				onClose={() => setAdding(false)}
			/>
		</Card>
	);
};

const CliGroup = ({
	group,
	budget,
	mac,
}: {
	group: CliSkills;
	budget: Budget | undefined;
	mac: boolean;
}) => {
	const { t } = useI18n();
	const hint = hideHint(group.cli);
	const fixed = group.skills.some((s) => s.place === "plugin" || s.place === "system");
	return (
		<CardBody>
			<View style={styles.cliHead}>
				<Text accessibilityRole="header" style={styles.cli}>
					{cliLabel(group.cli)}
				</Text>
				{budget ? <BudgetMeter budget={budget} /> : null}
			</View>
			{group.skills.map((skill) => (
				<View key={skill.path} style={styles.seen}>
					<SkillName
						name={skill.name}
						line={skill.mark ? markLabel(skill.mark) : null}
						warn={skill.mark === "conflict" || skill.mark === "shadowed"}
					/>
					<Badge label={originLabel(skill)} tone="plain" />
				</View>
			))}
			{fixed && hint ? <Muted>{hint}</Muted> : null}
			{group.cli === "cursor" && mac ? (
				<Muted>
					{t(
						"Cursor's own skills in ~/.cursor on this Mac aren't read, so they don't show here."
					)}
				</Muted>
			) : null}
		</CardBody>
	);
};

const Effective = ({
	project,
	device,
	place,
}: {
	project: ProjectId;
	device: DeviceId;
	place: AgentPlace | null;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const scan = useSkillScan(source, place ? device : null, place);
	const mac = view.tables.device.get(device)?.os === "macos";
	const skills = scan?.skills ?? null;
	const budgets = skills ? budgetsOf(view, project, skills) : [];
	return (
		<Card title={t("What each CLI sees")}>
			{!place ? (
				<CardBody>
					<Muted>
						{t("There's no checkout of this project on {{value1}} to look in.", {
							value1: deviceName(view, device),
						})}
					</Muted>
				</CardBody>
			) : (
				<CardBody>
					{skills === null ? (
						scan?.state === "failed" ? (
							<Text accessibilityRole="alert" style={[styles.state, styles.bad]}>
								{scan.error}
							</Text>
						) : (
							<Muted>{t("Looking")}</Muted>
						)
					) : null}
					<View style={styles.start}>
						<Button
							label={t("Rescan")}
							small
							disabled={scan?.state === "scanning"}
							onPress={() => void rescanSkills(source, device, place)}
						/>
					</View>
				</CardBody>
			)}
			{place && skills
				? skillsByCli(skills).map((group) => (
						<CliGroup
							key={group.cli}
							group={group}
							budget={budgets.find((b) => b.cli === group.cli)}
							mac={mac}
						/>
					))
				: null}
		</Card>
	);
};

const OnDevice = ({
	project,
	device,
	library,
}: {
	project: ProjectId;
	device: DeviceId;
	library: SkillLibrary;
}) => {
	const view = useView((s) => s.view);
	const place = useSteadyPlace(scanPlaceOf(view, project, device));
	return (
		<>
			<ThisProject project={project} device={device} library={library} place={place} />
			<Effective project={project} device={device} place={place} />
		</>
	);
};

export const SkillsScreen = ({ navigation, route }: ScreenProps<"Skills">) => {
	useI18n();
	const id = route.params.project;
	const view = useView((s) => s.view);
	const project = view.tables.project.get(id);
	const devices = skillDevices(view, id);
	const library = skillLibrary(view);
	const [picked, setPicked] = useState<DeviceId | null>(null);
	const device = shownDevice(devices, library, picked);

	return (
		<View style={styles.screen}>
			<Header
				title={t("Skills")}
				subtitle={
					project ? (
						<Text style={styles.sub} numberOfLines={1}>
							{project.name}
						</Text>
					) : undefined
				}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				automaticallyAdjustKeyboardInsets
			>
				{!project ? (
					<Muted>{t("That project is gone.")}</Muted>
				) : (
					<>
						<OutsideBanner project={id} />
						<DriftBanner project={id} />
						<Proposals
							project={id}
							onOpen={(proposal) =>
								navigation.navigate("SkillProposal", { proposal })
							}
						/>
						{!device ? (
							<Muted>
								{t(
									"A device keeps a project's skills once it hosts a workspace of it and runs a newer maxd."
								)}
							</Muted>
						) : (
							<>
								{devices.length > 1 ? (
									<Tabs
										tabs={devices.map((d) => ({
											value: d,
											label: deviceName(view, d),
										}))}
										value={device}
										onChange={setPicked}
									/>
								) : null}
								{library ? (
									<OnDevice
										key={device}
										project={id}
										device={device}
										library={library}
									/>
								) : (
									<Muted>{t("No device holds a skill library yet.")}</Muted>
								)}
							</>
						)}
						<Button
							label={t("Skill library")}
							tone="link"
							onPress={() => navigation.navigate("SkillLibrary")}
						/>
					</>
				)}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	sub: { color: color.mutedForeground, fontSize: size.meta },
	bannerText: { color: color.foreground, fontSize: size.small },
	drift: { gap: 6 },
	start: { alignSelf: "flex-start" },
	search: { gap: 6, paddingHorizontal: 14, paddingTop: 12, paddingBottom: 6 },
	list: { paddingHorizontal: 14, paddingBottom: 40, gap: 4 },
	pickRow: { flexDirection: "row", alignItems: "center", gap: 10, paddingVertical: 8 },
	row: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
	pack: { gap: 4 },
	path: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.pill },
	state: { color: color.secondaryForeground, fontSize: size.meta },
	warn: { color: color.busyText },
	bad: { color: color.errorText },
	log: {
		borderRadius: radius.sm,
		backgroundColor: color.subpanel,
		paddingHorizontal: 8,
		paddingVertical: 4,
		color: color.mutedForeground,
		fontFamily: font.mono,
		fontSize: size.pill,
	},
	cliHead: {
		flexDirection: "row",
		alignItems: "center",
		justifyContent: "space-between",
		gap: 12,
	},
	cli: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	seen: { flexDirection: "row", alignItems: "flex-start", gap: 8 },
});
