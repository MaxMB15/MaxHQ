// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pieces the phone's skills screens share, as the desktop's: a skill's
// badges, the meter for a CLI's listing, a tinted banner for checkouts to
// look at, a skill's name with its second line, and a proposal's row.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { SkillProposal } from "@maxhq/shared/bindings";
import type { Budget } from "@maxhq/shared/skills/effective";
import { badgeLabel, SKILL_BADGE_TONE, type SkillBadge } from "@maxhq/shared/skills/library";

import { color, font, radius, size, tint, TOUCH } from "../../theme";
import { Chevron } from "../../ui/Icon";
import { Badge } from "../test-hosts/parts";

/** A skill's badges in a row, or nothing for none. */
export const SkillBadges = ({ badges }: { badges: readonly SkillBadge[] }) => {
	useI18n();
	return badges.length > 0 ? (
		<View style={styles.badges}>
			{badges.map((badge) => (
				<Badge key={badge} label={badgeLabel(badge)} tone={SKILL_BADGE_TONE[badge]} />
			))}
		</View>
	) : null;
};

/** How much of a CLI's listing its skills take, or how many it lists when it has no size. */
export const BudgetMeter = ({ budget }: { budget: Budget }) => {
	const { t } = useI18n();
	const { limit, used, over, listed } = budget;
	if (limit === null) {
		return (
			<Text style={styles.meter}>
				{t("{{listed}} listed", { listed: formatNumber(listed) })}
			</Text>
		);
	}
	const percent = Math.min(100, (used / limit) * 100);
	const text = t("{{value1}} of {{value2}} characters", {
		value1: formatNumber(used),
		value2: formatNumber(limit),
	});
	return (
		<View
			accessible
			accessibilityLabel={t("Skill listing, {{text}}{{value2}}", {
				text: text,
				value2: over ? t(", over") : "",
			})}
			style={styles.meterRow}
		>
			<View style={styles.track}>
				<View
					style={[
						styles.fill,
						{
							width: `${percent}%`,
							backgroundColor: over ? color.error : color.primary,
						},
					]}
				/>
			</View>
			<Text style={styles.meter}>{text}</Text>
		</View>
	);
};

/** A tinted box for checkouts that need a look, as the desktop's amber banners. */
export const Banner = ({ label, children }: { label: string; children: ReactNode }) => (
	<View accessibilityLabel={label} style={styles.banner}>
		{children}
	</View>
);

/** A skill's name in mono, with a muted second line under it. */
export const SkillName = ({
	name,
	line,
	warn,
	children,
}: {
	name: string;
	line?: string | null;
	/** Amber for the second line, such as a skill another one shadows. */
	warn?: boolean;
	/** What sits on the name's right, such as its badges. */
	children?: ReactNode;
}) => (
	<View style={styles.skill}>
		<View style={styles.skillHead}>
			<Text style={styles.name} numberOfLines={1}>
				{name}
			</Text>
			{children}
		</View>
		{line ? (
			<Text style={[styles.line, warn && styles.warn]} numberOfLines={2}>
				{line}
			</Text>
		) : null}
	</View>
);

/** A waiting proposal's row, which opens it. */
export const ProposalRow = ({
	proposal,
	line,
	first,
	onPress,
}: {
	proposal: SkillProposal;
	/** Who wrote it and when, or where it is. */
	line: string;
	first?: boolean;
	onPress: () => void;
}) => {
	const { t } = useI18n();
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={`${proposal.name}, ${line}`}
			accessibilityHint={t("Review")}
			onPress={onPress}
			style={({ pressed }) => [
				styles.row,
				!first && styles.rowLine,
				pressed && styles.pressed,
			]}
		>
			<View style={styles.rowText}>
				<Text style={styles.name} numberOfLines={1}>
					{proposal.name}
				</Text>
				<Text style={styles.line} numberOfLines={2}>
					{line}
				</Text>
			</View>
			<Chevron dir="right" size={12} />
		</Pressable>
	);
};

const styles = StyleSheet.create({
	badges: { flexDirection: "row", flexWrap: "wrap", gap: 4 },
	meterRow: { flexDirection: "row", alignItems: "center", gap: 6 },
	track: {
		width: 48,
		height: 4,
		borderRadius: 2,
		overflow: "hidden",
		backgroundColor: color.track,
	},
	fill: { height: 4 },
	meter: { color: color.mutedForeground, fontSize: size.tiny, fontVariant: ["tabular-nums"] },
	banner: {
		gap: 10,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.busy, 0.45),
		backgroundColor: tint(color.busy, 0.1),
		paddingHorizontal: 14,
		paddingVertical: 12,
	},
	skill: { flex: 1, minWidth: 0, gap: 2 },
	skillHead: { flexDirection: "row", alignItems: "center", gap: 8, flexWrap: "wrap" },
	name: { flexShrink: 1, color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	line: { color: color.mutedForeground, fontSize: size.meta },
	warn: { color: color.busyText },
	row: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 12,
		paddingHorizontal: 14,
		paddingVertical: 10,
		paddingRight: 12,
	},
	rowLine: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	rowText: { flex: 1, gap: 2 },
	pressed: { backgroundColor: color.accent },
});
