// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a PR and a task look on the phone, as on the desktop: a short word
// on a tinted badge, a PR's state as one, its checks and review as marks
// VoiceOver reads in words, its size, a link out to the browser, and the
// banner for checks the GitHub App can't read yet.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import type { ReactNode } from "react";
import { Linking, Pressable, StyleSheet, Text, View } from "react-native";

import type { ChecksState, DeviceId, PullSnapshot, ReviewState } from "@maxhq/shared/bindings";
import { appInstallUrl, githubAppOn } from "@maxhq/shared/pulls/app";
import { checksLabel, PULL_STATE, pullStateKey, reviewLabel } from "@maxhq/shared/pulls/look";
import type { Tone } from "@maxhq/shared/tasks/format";

import { useView } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import {
	CircleCheck,
	CircleDashed,
	CircleDot,
	CircleQuestionMark,
	CircleX,
	ExternalLink,
	TriangleAlert,
	UserCheck,
	UserRound,
	UserRoundX,
} from "../../ui/glyphs";
import { opensInBrowser } from "../../ui/markdown";

const TONE: Record<Tone, { readonly box: string; readonly text: string }> = {
	good: { box: tint(color.ready, 0.15), text: color.readyText },
	warn: { box: tint(color.busy, 0.15), text: color.busyText },
	bad: { box: tint(color.error, 0.15), text: color.errorText },
	info: { box: tint(color.primary, 0.15), text: color.primary },
	plain: { box: color.secondary, text: color.mutedForeground },
};

/** A short word with a tone behind it. */
export const Badge = ({ tone, children }: { tone: Tone; children: string }) => (
	<View style={[styles.badge, { backgroundColor: TONE[tone].box }]}>
		<Text numberOfLines={1} style={[styles.badgeText, { color: TONE[tone].text }]}>
			{children}
		</Text>
	</View>
);

/** The text color of a tone, for a line that says how something went. */
export const toneText = (tone: Tone): string => TONE[tone].text;

/** Open, draft, merged or closed. */
export const PullStateBadge = ({ pull }: { pull: Pick<PullSnapshot, "state" | "draft"> }) => {
	useI18n();
	const look = PULL_STATE[pullStateKey(pull)];
	return <Badge tone={look.tone}>{look.label}</Badge>;
};

type Mark = { readonly icon: typeof CircleCheck; readonly color: string };

const CHECKS: Record<ChecksState, Mark> = {
	passing: { icon: CircleCheck, color: color.readyText },
	failing: { icon: CircleX, color: color.errorText },
	pending: { icon: CircleDot, color: color.busyText },
	none: { icon: CircleDashed, color: color.subtleForeground },
	unknown: { icon: CircleQuestionMark, color: color.mutedForeground },
};

const REVIEW: Record<ReviewState, Mark> = {
	approved: { icon: UserCheck, color: color.readyText },
	changes_requested: { icon: UserRoundX, color: color.busyText },
	none: { icon: UserRound, color: color.subtleForeground },
};

const MarkIcon = ({ mark, label }: { mark: Mark; label: string }) => (
	<View accessible accessibilityRole="image" accessibilityLabel={label}>
		<mark.icon size={14} strokeWidth={2.1} color={mark.color} />
	</View>
);

/** How the PR's checks stand, as an icon. */
export const ChecksMark = ({ state }: { state: ChecksState }) => {
	useI18n();
	return <MarkIcon mark={CHECKS[state]} label={checksLabel(state)} />;
};

/** Where the PR's reviews leave it, as an icon. */
export const ReviewMark = ({ state }: { state: ReviewState }) => {
	useI18n();
	return <MarkIcon mark={REVIEW[state]} label={reviewLabel(state)} />;
};

/** Lines added and deleted, or nothing while they aren't measured yet. */
export const PullSize = ({ pull }: { pull: Pick<PullSnapshot, "additions" | "deletions"> }) => {
	useI18n();
	return pull.additions + pull.deletions > 0 ? (
		<View style={styles.size}>
			<Text style={[styles.sizeText, { color: color.readyText }]}>
				+{formatNumber(pull.additions)}
			</Text>
			<Text style={[styles.sizeText, { color: color.errorText }]}>
				-{formatNumber(pull.deletions)}
			</Text>
		</View>
	) : null;
};

/** Opens a GitHub page in the browser. */
export const openOut = (url: string) => {
	if (opensInBrowser(url)) void Linking.openURL(url);
};

/** A link that opens in the browser, with its arrow. */
export const OutLink = ({ url, children }: { url: string; children: ReactNode }) => (
	<Pressable
		accessibilityRole="link"
		onPress={() => openOut(url)}
		hitSlop={10}
		style={({ pressed }) => [styles.out, pressed && styles.pressed]}
	>
		<Text style={styles.outText}>{children}</Text>
		<ExternalLink size={11} strokeWidth={2.2} color={color.primary} />
	</Pressable>
);

/**
 * Says the GitHub App on `device` needs its new permissions accepted on
 * the repository's owner before checks show, with a link to accept them
 * once GitHub has said the App's slug.
 */
export const UnknownChecksBanner = ({
	repo,
	device,
}: {
	repo: string;
	device: DeviceId | undefined;
}) => {
	const { t } = useI18n();
	const url = useView((s) => appInstallUrl(device ? githubAppOn(s.view, device) : undefined));
	const owner = repo.split("/")[0] ?? repo;
	return (
		<View style={styles.banner} accessibilityRole="summary">
			<TriangleAlert size={14} strokeWidth={2.1} color={color.busyText} />
			<Text style={styles.bannerText}>
				{t(
					"Accept the new permissions for your GitHub App on {{owner}} to see checks here",
					{
						owner: owner,
					}
				)}
			</Text>
			{url ? <OutLink url={url}>{t("Accept")}</OutLink> : null}
		</View>
	);
};

const styles = StyleSheet.create({
	badge: {
		minHeight: 20,
		justifyContent: "center",
		paddingHorizontal: 6,
		borderRadius: 5,
		flexShrink: 0,
	},
	badgeText: { fontSize: size.pill, fontWeight: "600" },
	size: { flexDirection: "row", gap: 4 },
	sizeText: { fontFamily: font.mono, fontSize: size.pill },
	out: { flexDirection: "row", alignItems: "center", gap: 4, alignSelf: "flex-start" },
	outText: { color: color.primary, fontSize: size.meta },
	pressed: { opacity: 0.6 },
	banner: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		padding: 10,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.busy, 0.4),
		backgroundColor: tint(color.busy, 0.08),
	},
	bannerText: { flex: 1, color: color.secondaryForeground, fontSize: size.tiny },
});
