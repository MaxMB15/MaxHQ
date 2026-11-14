// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A wait for a usage limit to reset. While it lasts, an agent MaxHQ holds can
// stop carrying on by itself once the limit resets.

import { useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import { isOpen, tookOf } from "@maxhq/shared/agent-view/format";
import {
	limitWaitBody,
	limitWaitEnded,
	limitWaitTitle,
	stopResume,
} from "@maxhq/shared/agent-view/limit-wait";
import type { TimelineItem } from "@maxhq/shared/bindings";
import { waitingMessages } from "@maxhq/shared/composer/send";
import { useNow } from "@maxhq/shared/lib/clock";
import { continuesAfterLimit } from "@maxhq/shared/settings/limits";

import { useServices, useView } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Clock } from "../../ui/glyphs";
import { useFeed } from "./feed-context";
import { Divider } from "./Notices";

const NONE_HIDDEN: ReadonlySet<string> = new Set();

const DontResume = ({
	waiting,
	carriesOn,
}: {
	waiting: readonly TimelineItem[];
	carriesOn: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { agent } = useFeed();
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);
	const dontResume = async () => {
		setBusy(true);
		setFailure(null);
		setFailure(await stopResume(source, agent, waiting, carriesOn));
		setBusy(false);
	};
	return (
		<View style={styles.actions}>
			<Pressable
				accessibilityRole="button"
				disabled={busy}
				hitSlop={9}
				onPress={() => void dontResume()}
				style={[styles.button, busy ? styles.busy : null]}
			>
				<Text style={styles.buttonText}>{t("Don't resume")}</Text>
			</Pressable>
			{failure ? (
				<Text accessibilityRole="alert" style={styles.failure}>
					{failure}
				</Text>
			) : null}
		</View>
	);
};

const Waiting = ({ item }: { item: Extract<TimelineItem, { kind: "limit_wait" }> }) => {
	const { t } = useI18n();

	const { store, readOnly, agent } = useFeed();
	const account = useView((s) =>
		item.account ? s.view.tables.account.get(item.account) : undefined
	);
	const carriesOn = useView((s) => {
		const record = s.view.tables.agent.get(agent);
		return record !== undefined && continuesAfterLimit(s.view, record);
	});
	const queued = useStore(store, (s) => s.queued);
	const now = useNow(1000);
	const waiting = useMemo(() => waitingMessages(queued, NONE_HIDDEN), [queued]);
	// maxd stops the resume once you take back the last message it would
	// resume with, and starts it again when you queue something new. Its
	// switch for continuing after the limit carries it on too.
	const resumes = item.auto_resume || carriesOn;
	return (
		<View accessibilityLabel={t("Waiting for usage limit reset")} style={styles.card}>
			<View style={styles.icon}>
				<Clock size={15} strokeWidth={1.9} color={color.busy} />
			</View>
			<View style={styles.words}>
				<Text style={styles.title}>{limitWaitTitle(item.resets_at)}</Text>
				<Text style={styles.body}>
					{limitWaitBody(account, resumes, item.resets_at, now)}
				</Text>
				{!readOnly && ((item.auto_resume && waiting.length > 0) || carriesOn) ? (
					<DontResume waiting={waiting} carriesOn={carriesOn} />
				) : null}
			</View>
		</View>
	);
};

/** A wait for a usage limit to reset. */
export const LimitWaitCard = ({
	item,
}: {
	item: Extract<TimelineItem, { kind: "limit_wait" }>;
}) => {
	return isOpen(item.status) ? (
		<Waiting item={item} />
	) : (
		<Divider icon={<Clock size={12} strokeWidth={1.9} color={color.mutedForeground} />}>
			{limitWaitEnded(tookOf(item))}
		</Divider>
	);
};

const styles = StyleSheet.create({
	card: {
		flexDirection: "row",
		alignItems: "flex-start",
		gap: 11,
		padding: 12,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.busy, 0.3),
		backgroundColor: tint(color.busy, 0.06),
	},
	icon: { marginTop: 2 },
	words: { flex: 1, minWidth: 0, gap: 4 },
	title: {
		color: color.foregroundStrong,
		fontSize: size.small,
		fontWeight: "600",
		fontVariant: ["tabular-nums"],
	},
	body: {
		color: color.secondaryForeground,
		fontSize: size.meta,
		lineHeight: 18,
		fontVariant: ["tabular-nums"],
	},
	actions: { marginTop: 4, gap: 6, alignItems: "flex-start" },
	button: {
		height: 26,
		justifyContent: "center",
		paddingHorizontal: 10,
		borderRadius: 6,
	},
	busy: { opacity: 0.5 },
	buttonText: { color: color.secondaryForeground, fontSize: size.meta },
	failure: { color: color.errorText, fontSize: size.tiny },
});
