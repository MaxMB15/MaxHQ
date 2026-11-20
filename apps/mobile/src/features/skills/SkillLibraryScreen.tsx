// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The skill library, as the desktop's settings page has it: every
// proposal waiting, as the desktop's sidebar lists them, the device that
// holds the library, each source with its trust, where it comes from and
// its skills with their badges, a form for a new source, the skills every
// agent gets, and for each device a tidy of its global skills with an
// Undo. Push, Fetch, Remove, Add and the tidy take the Mac's switch for
// the device they go to. Opened from the phone's Settings and from a
// project's Skills screen.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId, SkillLibrary, SkillProposalId, SkillSource } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { readOf } from "@maxhq/shared/pulls/read";
import {
	skillBadges,
	skillLibrary,
	skillProposals,
	sourceFacts,
} from "@maxhq/shared/skills/library";
import { devicesWithSkills, proposalTitle } from "@maxhq/shared/skills/project";
import { EMPTY_SOURCE, planSource, type SourceDraft } from "@maxhq/shared/skills/source-form";
import { deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { Header } from "../../ui/Header";
import { Buttons, CardBody, Muted, NoteLine, useAction } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { useRead } from "../pulls/read";
import { Hint, TextField } from "../tasks/fields";
import { Badge, GatedToggle } from "../test-hosts/parts";
import { ProposalRow, SkillBadges, SkillName } from "./look";
import { TidySheet } from "./TidySheet";

const Waiting = ({ onOpen }: { onOpen: (id: SkillProposalId) => void }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const proposals = skillProposals(view);
	if (proposals.length === 0) return null;
	return (
		<Card title={t("Proposals")} note={t("Changes agents want in your skills")}>
			{proposals.map((proposal) => (
				<ProposalRow
					key={proposal.id}
					proposal={proposal}
					line={proposalTitle(view, proposal)}
					onPress={() => onOpen(proposal.id)}
				/>
			))}
		</Card>
	);
};

const SourceBlock = ({
	library,
	source,
	lets,
}: {
	library: SkillLibrary;
	source: SkillSource;
	lets: boolean;
}) => {
	const { t } = useI18n();
	const { source: data } = useServices();
	const now = useNow(60_000);
	const { busy, note, run } = useAction();
	const off = busy || !lets;
	return (
		<View accessibilityLabel={source.name} style={styles.source}>
			<CardBody>
				<View style={styles.head}>
					<Text style={styles.sourceName} numberOfLines={1}>
						{source.name}
					</Text>
					<Badge
						label={source.trust === "own" ? t("Yours") : t("Third party")}
						tone={source.trust === "own" ? "good" : "plain"}
					/>
				</View>
				<Text style={styles.facts}>{sourceFacts(source, now).join(" · ")}</Text>
				{source.problem ? <Text style={styles.problem}>{source.problem}</Text> : null}
				<Buttons>
					{source.unpushed ? (
						<Button
							label={t("Push")}
							small
							disabled={off}
							onPress={() =>
								void run(
									t("Pushing"),
									() => data.pushSkillSource(library.device, source.id),
									t("Pushed")
								)
							}
						/>
					) : null}
					<Button
						label={t("Fetch")}
						small
						disabled={off}
						onPress={() =>
							void run(
								t("Fetching"),
								() => data.fetchSkillSource(library.device, source.id),
								t("Fetched")
							)
						}
					/>
					<Button
						label={t("Remove")}
						tone="danger"
						small
						disabled={off}
						onPress={() =>
							confirm(
								{ title: t("Remove {{name}}?", { name: source.name }), detail: "" },
								t("Remove"),
								() =>
									void run(
										t("Removing"),
										() => data.removeSkillSource(library.device, source.id),
										t("Removed")
									)
							)
						}
					/>
				</Buttons>
				<NoteLine note={note} />
			</CardBody>
			{source.skills.length === 0 ? (
				<CardBody>
					<Muted>{t("No skills found in it yet.")}</Muted>
				</CardBody>
			) : (
				<View
					accessibilityLabel={t("Skills in {{name}}", { name: source.name })}
					style={styles.skills}
				>
					{source.skills.map((skill) => (
						<SkillName key={skill.name} name={skill.name} line={skill.description}>
							<SkillBadges badges={skillBadges(skill, source)} />
						</SkillName>
					))}
				</View>
			)}
		</View>
	);
};

const AddSource = ({ library, lets }: { library: SkillLibrary; lets: boolean }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const [draft, setDraft] = useState<SourceDraft>(EMPTY_SOURCE);
	const { busy, note, setNote, run } = useAction();
	const plan = planSource(library, draft);
	const set = (change: Partial<SourceDraft>) => {
		setNote(null);
		setDraft({ ...draft, ...change });
	};
	const add = async () => {
		if (!plan.ok) return;
		const added = await run(
			t("Adding"),
			() =>
				source.addSkillSource(library.device, plan.name, plan.origin, plan.ref, plan.trust),
			t("Added")
		);
		if (added) setDraft(EMPTY_SOURCE);
	};
	return (
		<Card title={t("Add a source")}>
			<CardBody first>
				<TextField
					label={t("Repo or folder")}
					value={draft.where}
					placeholder={t("owner/repo, a git URL, or /path/to/skills")}
					mono
					onChange={(where) => set({ where })}
				/>
				<TextField
					label={t("Name")}
					value={draft.name}
					placeholder={t("From the repo")}
					onChange={(name) => set({ name })}
				/>
				<TextField
					label={t("Branch or tag")}
					value={draft.ref}
					placeholder={t("Its default branch")}
					mono
					onChange={(ref) => set({ ref })}
				/>
			</CardBody>
			<CardRow
				label={t("A third party's skills")}
				note={t("A third party's skills, pinned until you update them")}
			>
				<GatedToggle
					label={t("A third party's skills")}
					value={draft.thirdParty}
					disabled={false}
					onChange={(thirdParty) => set({ thirdParty })}
				/>
			</CardRow>
			<CardBody>
				{!plan.ok && draft.where.trim() ? <Hint>{plan.reason}</Hint> : null}
				<Button
					label={t("Add source")}
					tone="primary"
					disabled={!plan.ok || busy || !lets}
					onPress={() => void add()}
				/>
				<NoteLine note={note} />
				{!lets ? <NeedsSwitch /> : null}
			</CardBody>
		</Card>
	);
};

/** A device's row under Global skills: Undo for its last tidy, and the tidy. */
const DeviceTidy = ({
	device,
	again,
	first,
	onTidy,
}: {
	device: DeviceId;
	/** Counts the tidies, so the row reads its plan again after one. */
	again: number;
	first: boolean;
	onTidy: () => void;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const name = useView((s) => deviceName(s.view, device));
	const lets = useLetsPhone(device);
	const { busy, note, run } = useAction();
	const [undone, setUndone] = useState(0);
	const plan = useRead(
		`plan\n${device}`,
		async () => readOf(await source.planGlobalSkills(device), "skills_globals_plan").plan,
		again + undone
	);
	const undo = async () => {
		const put = await run(
			t("Putting back"),
			() => source.undoGlobalSkills(device),
			t("Put back")
		);
		if (put) setUndone((n) => n + 1);
	};
	return (
		<CardBody first={first}>
			<Text style={styles.device}>{name}</Text>
			<Buttons>
				{plan.data?.undo ? (
					<Button
						label={t("Undo")}
						small
						disabled={busy || !lets}
						onPress={() => void undo()}
					/>
				) : null}
				<Button label={t("Tidy global skills")} small onPress={onTidy} />
			</Buttons>
			<NoteLine note={note} />
			{plan.data?.undo && !lets ? <NeedsSwitch /> : null}
		</CardBody>
	);
};

export const SkillLibraryScreen = ({ navigation }: ScreenProps<"SkillLibrary">) => {
	useI18n();
	const view = useView((s) => s.view);
	const library = skillLibrary(view);
	const devices = devicesWithSkills(view);
	const lets = useLetsPhone(library?.device ?? null);
	const [tidying, setTidying] = useState<DeviceId | null>(null);
	const [tidied, setTidied] = useState<string | null>(null);
	const [tidies, setTidies] = useState(0);

	return (
		<View style={styles.screen}>
			<Header
				title={t("Skill library")}
				subtitle={
					library ? (
						<Text style={styles.sub} numberOfLines={1}>
							{t("Held by {{value1}}", { value1: deviceName(view, library.device) })}
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
				<Waiting
					onOpen={(proposal) => navigation.navigate("SkillProposal", { proposal })}
				/>
				{!library ? (
					<Muted>
						{t("No device holds a skill library yet. It comes with a newer maxd.")}
					</Muted>
				) : (
					<>
						<Card title={t("Sources")}>
							{library.sources.length === 0 ? (
								<CardBody>
									<Muted>
										{t("No sources yet. Add a repo or folder of skills below.")}
									</Muted>
								</CardBody>
							) : (
								library.sources.map((source) => (
									<SourceBlock
										key={source.id}
										library={library}
										source={source}
										lets={lets}
									/>
								))
							)}
							{!lets ? (
								<CardBody>
									<NeedsSwitch />
								</CardBody>
							) : null}
						</Card>
						<AddSource library={library} lets={lets} />
						<Card
							title={t("Every agent")}
							note={t("Global on each device, so projects can't take them")}
						>
							<CardBody>
								{library.always.length === 0 ? (
									<Muted>{t("None yet.")}</Muted>
								) : (
									<View
										accessibilityLabel={t("Skills every agent gets")}
										style={styles.chips}
									>
										{library.always.map((name) => (
											<Text key={name} style={styles.chip}>
												{name}
											</Text>
										))}
									</View>
								)}
							</CardBody>
						</Card>
						<Card
							title={t("Global skills")}
							note={t("Move your own into the library, device by device")}
						>
							{devices.map((device, i) => (
								<DeviceTidy
									key={device}
									device={device}
									again={tidies}
									first={i === 0}
									onTidy={() => {
										setTidied(null);
										setTidying(device);
									}}
								/>
							))}
							{tidied ? (
								<CardBody>
									<Text accessibilityLiveRegion="polite" style={styles.facts}>
										{tidied}
									</Text>
								</CardBody>
							) : null}
						</Card>
					</>
				)}
			</ScrollView>
			<TidySheet
				device={tidying}
				onClose={() => setTidying(null)}
				onDone={() => {
					if (tidying) {
						setTidied(
							t("Tidied {{value1}}. Undo puts it back.", {
								value1: deviceName(view, tidying),
							})
						);
					}
					setTidies((n) => n + 1);
					setTidying(null);
				}}
			/>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	sub: { color: color.mutedForeground, fontSize: size.meta },
	source: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	head: { flexDirection: "row", alignItems: "center", gap: 8 },
	sourceName: { flexShrink: 1, color: color.foreground, fontSize: size.small, fontWeight: "600" },
	facts: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.meta },
	problem: { color: color.errorText, fontSize: size.meta },
	skills: { gap: 10, paddingHorizontal: 14, paddingBottom: 12 },
	device: { color: color.foreground, fontSize: size.small },
	chips: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	chip: {
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.secondary,
		paddingHorizontal: 8,
		paddingVertical: 2,
		overflow: "hidden",
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.meta,
	},
});
