// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A PR's Summary tab, as on the desktop: its description, each reviewer's
// latest verdict, whether it merges cleanly, and Merge and Approve as you.
// Each action is disabled with its reason on a line under the buttons, and
// the merge method is picked from a sheet. With no sign-in, or with the
// device holding it offline, the two become one Open on GitHub button. A
// merge or an approval drops what was read of every PR, so it reads again.

import { useI18n } from "@maxhq/shared/i18n";

import { type ReactElement, type ReactNode, useState } from "react";
import { type RefreshControlProps, ScrollView, StyleSheet, Text, View } from "react-native";

import type { MergeMethod, PullDetail, PullReview } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	approvePull,
	approveUnavailableReason,
	mergePull,
	type PullActed,
} from "@maxhq/shared/pulls/act";
import { githubSignIn } from "@maxhq/shared/pulls/devices";
import {
	allowedMergeMethods,
	MERGE_METHODS,
	mergeUnavailableReason,
} from "@maxhq/shared/pulls/merge";
import { actedNote, latestVerdicts, mergeability, VERDICT } from "@maxhq/shared/pulls/review";
import { pullUrl } from "@maxhq/shared/pulls/urls";

import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose } from "../../ui/choose";
import { ChevronDown } from "../../ui/glyphs";
import { MarkdownText } from "../../ui/MarkdownText";
import { openOut } from "./look";
import { dropPulls } from "./read";

const Section = ({ title, children }: { title: string; children: ReactNode }) => (
	<View style={styles.section} accessibilityLabel={title}>
		<Text style={styles.heading} accessibilityRole="header">
			{title}
		</Text>
		{children}
	</View>
);

const VERDICT_TEXT: Record<PullReview["verdict"], string> = {
	approved: color.readyText,
	changes_requested: color.busyText,
	commented: color.mutedForeground,
	dismissed: color.mutedForeground,
};

const Reviews = ({ reviews }: { reviews: readonly PullReview[] }) => {
	const { t } = useI18n();

	const shown = latestVerdicts(reviews);
	if (shown.length === 0) return <Text style={styles.quiet}>{t("No reviews yet")}</Text>;
	return (
		<View style={styles.reviews}>
			{shown.map((review) => (
				<View key={review.author} style={styles.review}>
					<Text style={styles.reviewer} numberOfLines={1}>
						{review.author}
					</Text>
					<Text style={[styles.verdict, { color: VERDICT_TEXT[review.verdict] }]}>
						{VERDICT[review.verdict]}
					</Text>
				</View>
			))}
		</View>
	);
};

type Note = { readonly text: string; readonly error: boolean };

const Actions = ({ pr }: { pr: PullDetail }) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const signIn = githubSignIn(view, now);
	const [busy, setBusy] = useState<"merge" | "approve" | null>(null);
	const [picked, setPicked] = useState<MergeMethod | null>(null);
	const [note, setNote] = useState<Note | null>(null);

	const methods = allowedMergeMethods(pr);
	const method = picked && methods.includes(picked) ? picked : methods[0];
	const mergeWhy = mergeUnavailableReason(pr, signIn);
	const approveWhy = approveUnavailableReason(pr, signIn);
	const at = { key: pr.summary.key, headSha: pr.summary.pull.head_sha };

	const run = (act: "merge" | "approve", acting: () => Promise<PullActed>) => {
		setNote(null);
		setBusy(act);
		acting()
			.then(
				(acted) => setNote(actedNote(act, acted)),
				(error: unknown) =>
					setNote({
						text: error instanceof Error ? error.message : String(error),
						error: true,
					})
			)
			.finally(() => {
				setBusy(null);
				dropPulls();
			});
	};
	const merge = () => {
		if (!method) return;
		run("merge", () =>
			mergePull(services.source, services.view.getState().view, Date.now(), at, method)
		);
	};
	const approve = () =>
		run("approve", () =>
			approvePull(services.source, services.view.getState().view, Date.now(), at)
		);
	const pickMethod = () =>
		choose(
			t("Merge method"),
			methods.map((one) => ({
				label: MERGE_METHODS[one].title,
				disabled: one === method,
				pick: () => setPicked(one),
			})),
			method ? MERGE_METHODS[method].description : undefined
		);

	if (signIn.state !== "ready") {
		return (
			<View style={styles.actions}>
				<View style={styles.row}>
					<Button
						label={t("Open on GitHub")}
						onPress={() => openOut(pullUrl(pr.summary.key))}
					/>
				</View>
				{mergeWhy ? <Text style={styles.why}>{mergeWhy}</Text> : null}
			</View>
		);
	}

	const reasons = [...new Set([mergeWhy, approveWhy].filter((why) => why !== null))];
	return (
		<View style={styles.actions}>
			<View style={styles.row}>
				<Button
					label={
						busy === "merge"
							? t("Merging")
							: method
								? MERGE_METHODS[method].title
								: t("Merge")
					}
					tone="primary"
					grow={1}
					disabled={mergeWhy !== null || busy !== null || !method}
					onPress={merge}
				/>
			</View>
			<View style={styles.row}>
				<Button
					label={t("Merge method")}
					icon={<ChevronDown size={15} strokeWidth={2.4} color={color.foreground} />}
					grow={1}
					disabled={mergeWhy !== null || busy !== null}
					onPress={pickMethod}
				/>
				<Button
					label={busy === "approve" ? t("Approving") : t("Approve")}
					grow={1}
					disabled={approveWhy !== null || busy !== null}
					onPress={approve}
				/>
			</View>
			{reasons.map((why) => (
				<Text key={why} style={styles.why}>
					{why}
				</Text>
			))}
			{note ? (
				<Text
					style={[styles.note, note.error && styles.error]}
					accessibilityRole={note.error ? "alert" : undefined}
					accessibilityLiveRegion="polite"
				>
					{note.text}
				</Text>
			) : null}
		</View>
	);
};

/** The Summary tab of `pr`, with `refresh` to read it again. */
export const PullSummaryTab = ({
	pr,
	refresh,
}: {
	pr: PullDetail;
	refresh: ReactElement<RefreshControlProps>;
}) => {
	const { t } = useI18n();

	const mergeWords = mergeability(pr);
	return (
		<ScrollView contentContainerStyle={styles.content} refreshControl={refresh}>
			{pr.body?.trim() ? (
				<MarkdownText text={pr.body} />
			) : (
				<Text style={styles.quiet}>{t("No description.")}</Text>
			)}
			<Section title={t("Reviews")}>
				<Reviews reviews={pr.reviews} />
			</Section>
			{mergeWords ? (
				<Section title={t("Merging")}>
					<Text
						style={[
							styles.quiet,
							pr.summary.pull.mergeable === "conflicting" && styles.error,
						]}
					>
						{mergeWords}
					</Text>
				</Section>
			) : null}
			<Actions pr={pr} />
		</ScrollView>
	);
};

const styles = StyleSheet.create({
	content: { gap: 18, padding: 14, paddingBottom: 40 },
	section: { gap: 6 },
	heading: { color: color.secondaryForeground, fontSize: size.tiny, fontWeight: "600" },
	quiet: { color: color.mutedForeground, fontSize: size.small },
	error: { color: color.errorText },
	reviews: { gap: 4 },
	review: { flexDirection: "row", alignItems: "center", gap: 8 },
	reviewer: { flexShrink: 1, color: color.foreground, fontSize: size.small },
	verdict: { fontSize: size.small },
	actions: { gap: 8 },
	row: { flexDirection: "row", gap: 8 },
	why: { color: color.mutedForeground, fontSize: size.tiny },
	note: { color: color.readyText, fontSize: size.small },
});
