// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A skill change an agent proposes, as the desktop's viewer shows it: who
// wrote it, where and when, what the changed skill does beyond its
// instructions, its changed files in the phone's diff view, then Reject,
// Approve and push, or Approve. A third party's skill asks for a fork's
// name first. Approve waits for the diff and carries what it showed, so
// an approve can't land on anything else. Answering takes the Mac's switch
// for the proposal's device, and for an approve also for the device that
// holds the library.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import type { ProposalAction, ProposalDiff, SkillProposal } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { readOf } from "@maxhq/shared/pulls/read";
import { skillBadges, skillLibrary, skillProposals } from "@maxhq/shared/skills/library";
import { answerDone, proposalReview, proposalTitle } from "@maxhq/shared/skills/project";
import { formatAgo } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { Buttons, Muted, NoteLine, useAction } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { FileRow, Quiet } from "../pulls/PullTabs";
import { useRead } from "../pulls/read";
import { Label } from "../tasks/fields";
import { SkillBadges } from "./look";
import { approveHolder, mayAnswer } from "./skills";

const Changes = ({ diff, error }: { diff: ProposalDiff | undefined; error: string | null }) => {
	const { t } = useI18n();
	if (!diff) {
		return error ? (
			<Text accessibilityRole="alert" style={styles.alert}>
				{t("Couldn't read the change. {{message}}", { message: error })}
			</Text>
		) : (
			<Quiet>{t("Reading the change")}</Quiet>
		);
	}
	if (diff.files.length === 0) return <Quiet>{t("No files changed")}</Quiet>;
	return (
		<View accessibilityLabel={t("Files")} style={styles.files}>
			{diff.files.map((file) => (
				<FileRow
					key={file.path}
					file={file}
					noPatch={t("There's no text diff for this file.")}
				/>
			))}
		</View>
	);
};

const Body = ({ proposal, onDone }: { proposal: SkillProposal; onDone: () => void }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const read = useRead(
		`${proposal.id}\n${proposal.tree}`,
		async () => readOf(await source.skillProposalDiff(proposal.id), "skill_proposal_diff").diff
	);
	const refresh = usePullToRefresh(read.reload);
	const diff = read.data;
	const [opened] = useState(proposal.tree);
	const [fork, setFork] = useState("");
	const { busy, note, run } = useAction();
	const library = skillLibrary(view);
	const { from, forking, refusal, base, seen, standing } = proposalReview(
		library,
		proposal,
		diff,
		fork
	);
	const holder = approveHolder(library, proposal);
	const letsHere = useLetsPhone(proposal.device);
	const letsHolder = useLetsPhone(holder);
	const mayReject = mayAnswer("reject", holder, letsHere, letsHolder);
	const mayApprove = mayAnswer("approve", holder, letsHere, letsHolder);
	const cantYet = busy || refusal !== null || seen === undefined || !mayApprove;

	const answer = async (action: ProposalAction, push: boolean) => {
		const named = action === "approve" && forking ? fork.trim() : undefined;
		const told = action === "approve" ? seen : undefined;
		const sent = await run(
			t("Sending"),
			() => source.answerSkillProposal(proposal.id, action, push, named, told),
			answerDone(action, push)
		);
		if (sent) onDone();
	};

	return (
		<ScrollView
			contentContainerStyle={styles.content}
			keyboardShouldPersistTaps="handled"
			automaticallyAdjustKeyboardInsets
			refreshControl={refresh}
		>
			<View style={styles.top}>
				<View style={styles.nameRow}>
					<Text style={styles.name}>{proposal.name}</Text>
					{standing ? (
						<Text style={[styles.standing, base !== null && styles.bad]}>
							{standing}
						</Text>
					) : null}
				</View>
				<SkillBadges badges={skillBadges(diff ?? proposal, from)} />
				<Text style={styles.meta}>
					{t("In {{checkout}} on {{value2}}, {{value3}}", {
						checkout: proposal.checkout,
						value2: deviceName(view, proposal.device),
						value3: formatAgo(proposal.created_at, now),
					})}
				</Text>
				{from ? (
					<Text style={styles.meta}>{t("From {{name}}", { name: from.name })}</Text>
				) : null}
			</View>
			<View style={styles.changes}>
				<Changes diff={diff} error={read.error} />
			</View>
			{proposal.tree !== opened ? (
				<Text accessibilityRole="alert" style={styles.alert}>
					{t(
						"This proposal changed since you opened it. What's above is the new version."
					)}
				</Text>
			) : null}
			{forking ? (
				<View style={styles.field}>
					<Label>{t("Fork name in your personal source")}</Label>
					<Input
						accessibilityLabel={t("Fork name in your personal source")}
						value={fork}
						onChangeText={setFork}
						placeholder={`my-${proposal.name}`}
						autoCapitalize="none"
						autoCorrect={false}
						spellCheck={false}
						style={styles.mono}
					/>
				</View>
			) : null}
			{refusal ? <Muted>{refusal}</Muted> : null}
			<View style={styles.answer}>
				<Buttons>
					<Button
						label={t("Reject")}
						tone="danger"
						grow={1}
						disabled={busy || !mayReject}
						onPress={() => void answer("reject", false)}
					/>
					<Button
						label={t("Approve")}
						tone="primary"
						grow={1}
						disabled={cantYet}
						onPress={() => void answer("approve", false)}
					/>
				</Buttons>
				<Button
					label={t("Approve and push")}
					disabled={cantYet}
					onPress={() => void answer("approve", true)}
				/>
				<NoteLine note={note} />
				{!mayApprove ? (
					<>
						<NeedsSwitch />
						{holder !== null && !letsHolder ? (
							<Muted>
								{t("Approving also reaches {{name}}, which holds the library.", {
									name: deviceName(view, holder),
								})}
							</Muted>
						) : null}
					</>
				) : null}
			</View>
		</ScrollView>
	);
};

/** One proposal, from the project's Skills screen or the skill library. */
export const SkillProposalScreen = ({ navigation, route }: ScreenProps<"SkillProposal">) => {
	const { t } = useI18n();
	const id = route.params.proposal;
	const view = useView((s) => s.view);
	const proposal = skillProposals(view).find((p) => p.id === id);
	return (
		<View style={styles.screen}>
			<Header
				title={proposal ? proposalTitle(view, proposal) : t("Skill change")}
				subtitle={
					proposal ? (
						<Text style={styles.sub} numberOfLines={1}>
							{view.tables.project.get(proposal.project)?.name ?? proposal.project}
						</Text>
					) : undefined
				}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			{proposal ? (
				<Body key={proposal.id} proposal={proposal} onDone={() => navigation.goBack()} />
			) : (
				<View style={styles.content}>
					<Muted>
						{t(
							"This proposal is gone. Someone answered it, or its agent took it back."
						)}
					</Muted>
				</View>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	sub: { color: color.mutedForeground, fontSize: size.meta },
	top: { gap: 6 },
	nameRow: { flexDirection: "row", alignItems: "center", flexWrap: "wrap", gap: 8 },
	name: { color: color.foregroundStrong, fontFamily: font.mono, fontSize: size.body },
	standing: { color: color.mutedForeground, fontSize: size.meta },
	bad: { color: color.errorText },
	meta: { color: color.mutedForeground, fontSize: size.meta },
	changes: {
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
		padding: 12,
	},
	files: { gap: 16 },
	alert: { color: color.errorText, fontSize: size.meta },
	field: { gap: 6 },
	mono: { fontFamily: font.mono, fontSize: size.meta },
	answer: { gap: 8 },
});
