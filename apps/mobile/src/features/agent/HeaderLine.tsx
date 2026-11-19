// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The second line of an agent's header: its activity, vendor, device and
// project, then how full its context window is as a small bar. A tap opens
// the agent's details, which hold the rest of what the desktop's header
// shows, so the header stays two calm lines. Under the header, a chip says
// when changed skills restart the agent, as the desktop's strip does.

import { useI18n } from "@maxhq/shared/i18n";

import { Pressable, StyleSheet, Text, View } from "react-native";

import type { Agent, AgentId } from "@maxhq/shared/bindings";
import { waitsForSkills } from "@maxhq/shared/skills/library";
import type { ActivityLook } from "@maxhq/shared/status/activity";
import { formatContext } from "@maxhq/shared/status/format";
import { contextShare } from "@maxhq/shared/status/sub-agents";

import { useView } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { ActivityDot } from "../../ui/Dot";
import { ChevronRight, RefreshCw } from "../../ui/glyphs";

// The title above counts as part of the line, so the target reaches 44 points.
const SLOP = { top: 22, bottom: 8 };

export const PlaceLine = ({
	agent,
	look,
	place,
	onPress,
}: {
	agent: Agent;
	look: ActivityLook;
	/** Its activity, vendor, device and project, as one line. */
	place: string;
	onPress: () => void;
}) => {
	const { t } = useI18n();

	const share = contextShare(agent.context);
	const context =
		agent.context && share !== null
			? t("Context window, {{text}} used", {
					text: formatContext(agent.context.used_tokens, agent.context.window_tokens),
				})
			: null;
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={context ? `${place}. ${context}` : place}
			accessibilityHint={t("Opens the agent's details")}
			hitSlop={SLOP}
			onPress={onPress}
			style={({ pressed }) => [styles.place, pressed && styles.pressed]}
		>
			<ActivityDot look={look} decorative />
			<Text style={styles.placeText} numberOfLines={1}>
				{place}
			</Text>
			{share !== null ? (
				<View style={styles.track}>
					<View
						style={[styles.fill, { width: `${share}%` }, share >= 85 && styles.full]}
					/>
				</View>
			) : null}
			<ChevronRight size={13} strokeWidth={2} color={color.mutedForeground} />
		</Pressable>
	);
};

/** The skills chip's bar, while a Cursor or ACP agent waits to restart for changed skills. */
export const SkillsChip = ({ agent }: { agent: AgentId }) => {
	const { t } = useI18n();

	const waits = useView((s) => waitsForSkills(s.view, agent));
	if (!waits) return null;
	return (
		<View style={styles.bar}>
			<View style={styles.chip}>
				<RefreshCw size={11} strokeWidth={2} color={color.busyText} />
				<Text style={styles.chipText} numberOfLines={1}>
					{t("Skills changed, restarts on next message")}
				</Text>
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	place: {
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		marginTop: 1,
		borderRadius: radius.sm,
	},
	pressed: { opacity: 0.6 },
	placeText: { flexShrink: 1, color: color.mutedForeground, fontSize: size.meta },
	track: {
		width: 28,
		height: 3,
		borderRadius: 2,
		overflow: "hidden",
		backgroundColor: color.track,
	},
	fill: { height: 3, backgroundColor: color.primary },
	full: { backgroundColor: color.busy },
	bar: {
		paddingHorizontal: 14,
		paddingVertical: 6,
		borderBottomColor: color.border,
		borderBottomWidth: StyleSheet.hairlineWidth,
	},
	chip: {
		alignSelf: "flex-start",
		height: 22,
		flexDirection: "row",
		alignItems: "center",
		gap: 5,
		paddingHorizontal: 7,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.busy, 0.45),
		backgroundColor: tint(color.busy, 0.1),
	},
	chipText: { flexShrink: 1, color: color.busyText, fontSize: size.tiny },
});
