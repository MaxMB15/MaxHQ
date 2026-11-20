// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent list's skill changes, as the desktop's sidebar shows them: a
// row for each proposal waiting for you, newest first, which opens it.
// The list leaves them out while none wait.

import { useI18n } from "@maxhq/shared/i18n";

import { memo } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { SkillProposal, SkillProposalId } from "@maxhq/shared/bindings";
import { proposalTitle } from "@maxhq/shared/skills/project";
import { plural } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";

import { color, font, radius, size } from "../../theme";
import { Sparkles } from "../../ui/glyphs";

// As tall as the list's agent rows, and as wide, so it's easy to hit.
const ROW = 40;

/** The proposals waiting for you, as `skillProposals` gives them, each opening its screen. */
export const SkillChanges = memo(function SkillChanges({
	view,
	proposals,
	onOpen,
}: {
	view: ViewState;
	proposals: readonly SkillProposal[];
	onOpen: (proposal: SkillProposalId) => void;
}) {
	useI18n();
	return (
		<View accessibilityLabel={plural(proposals.length, "skill change")} style={styles.box}>
			{proposals.map((proposal) => {
				const title = proposalTitle(view, proposal);
				return (
					<Pressable
						key={proposal.id}
						accessibilityRole="button"
						accessibilityLabel={`${title}, ${proposal.name}`}
						onPress={() => onOpen(proposal.id)}
						style={({ pressed }) => [styles.row, pressed && styles.pressed]}
					>
						<Sparkles size={13} strokeWidth={2} color={color.secondaryForeground} />
						<Text style={styles.title} numberOfLines={1}>
							{title}
						</Text>
						<Text style={styles.name} numberOfLines={1}>
							{proposal.name}
						</Text>
					</Pressable>
				);
			})}
		</View>
	);
});

const styles = StyleSheet.create({
	box: { marginBottom: 8, gap: 2 },
	row: {
		minHeight: ROW,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 8,
		borderRadius: radius.sm,
	},
	pressed: { backgroundColor: color.accent },
	title: { flex: 1, minWidth: 0, color: color.secondaryForeground, fontSize: size.small },
	name: {
		flexShrink: 1,
		maxWidth: "40%",
		color: color.mutedForeground,
		fontFamily: font.mono,
		fontSize: size.meta,
	},
});
