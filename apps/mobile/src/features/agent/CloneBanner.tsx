// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Over the composer while an agent's folder is still cloning, with how far
// git has got and Cancel, or once the clone failed or was cancelled, with
// Remove and Retry, as the desktop has it. Agents can't start in the folder
// until the clone is done. Remove and Retry need the phone's switch on the
// folder's device; Cancel doesn't, since it only stops work. Below that, the
// files a project copy couldn't place in the folder, until you dismiss them,
// which needs no switch, since it only forgets them on this phone.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import { failureOf, type SendState } from "@maxhq/shared/agent-view/answer";
import type { WorkspaceId } from "@maxhq/shared/bindings";
import type { ActionResult, CloneState, PlaceFailure } from "@maxhq/shared/data/source";
import { sentence } from "@maxhq/shared/status/format";
import { dismissMissed, missedTitle, useMissed } from "@maxhq/shared/workspaces/missed";
import { cloneOf, cloneWording } from "@maxhq/shared/workspaces/state";

import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";

/** The clone's state, with what you can do about it. */
const Clone = ({ workspace, clone }: { workspace: WorkspaceId; clone: CloneState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [state, setState] = useState<SendState>({ state: "idle" });
	const device = useView((s) => s.view.tables.workspace.get(workspace)?.device ?? null);
	const lets = useLetsPhone(device);
	const { title, body } = cloneWording(clone);
	const busy = state.state === "sending";

	const run = (action: (id: WorkspaceId) => Promise<ActionResult>, fallback: string) => {
		setState({ state: "sending" });
		action(workspace).then(
			(result) => {
				const failure = failureOf(result);
				setState(failure ? { state: "failed", message: failure } : { state: "idle" });
			},
			(error: unknown) =>
				setState({
					state: "failed",
					message: error instanceof Error && error.message ? error.message : fallback,
				})
		);
	};
	const percent = clone.state === "cloning" ? clone.percent : null;

	return (
		<View style={styles.box} accessibilityLabel={t("Clone")}>
			<Text style={styles.title}>{title}</Text>
			<Text style={styles.body}>{body}</Text>
			{clone.state === "cloning" ? (
				<View
					accessible
					accessibilityRole="progressbar"
					accessibilityLabel={t("Clone")}
					accessibilityValue={
						percent === null ? undefined : { min: 0, max: 100, now: percent }
					}
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
			) : null}
			{state.state === "failed" ? (
				<Text style={styles.failed} accessibilityRole="alert">
					{state.message}
				</Text>
			) : null}
			<View style={styles.actions}>
				{clone.state === "cloning" ? (
					<Button
						label={t("Cancel")}
						small
						disabled={busy}
						onPress={() => run(source.cancelClone, t("Couldn't cancel the clone."))}
					/>
				) : (
					<>
						<Button
							label={t("Remove")}
							small
							disabled={busy || !lets}
							onPress={() =>
								run(source.removeClone, t("Couldn't remove the folder."))
							}
						/>
						<Button
							label={t("Retry")}
							tone="primary"
							small
							disabled={busy || !lets}
							onPress={() => run(source.retryClone, t("Couldn't retry the clone."))}
						/>
					</>
				)}
			</View>
			{clone.state !== "cloning" && !lets ? <NeedsSwitch /> : null}
		</View>
	);
};

/** The files a copy couldn't place, until you dismiss them. */
const Missed = ({
	workspace,
	failed,
}: {
	workspace: WorkspaceId;
	failed: readonly PlaceFailure[];
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	return (
		<View style={styles.box} accessibilityLabel={t("Files not copied")}>
			<Text style={styles.title}>{missedTitle(failed.length)}</Text>
			<ScrollView style={styles.list} nestedScrollEnabled>
				{failed.map((f) => (
					<View key={f.path} style={styles.missed}>
						<Text numberOfLines={1} ellipsizeMode="middle" style={styles.path}>
							{f.path}
						</Text>
						<Text style={styles.body}>{sentence(f.reason)}</Text>
					</View>
				))}
			</ScrollView>
			<View style={styles.actions}>
				<Button
					label={t("Dismiss")}
					small
					onPress={() => dismissMissed(source, workspace)}
				/>
			</View>
		</View>
	);
};

/**
 * A workspace's clone banner, or nothing once the clone is done and every
 * file it was copied with landed.
 */
export const CloneBanner = ({ workspace }: { workspace: WorkspaceId }) => {
	const { source } = useServices();
	const clone = useView((s) => {
		const found = s.view.tables.workspace.get(workspace);
		return found ? cloneOf(found) : null;
	});
	const missed = useMissed(source, workspace);
	if (!clone && missed.length === 0) return null;
	return (
		<View style={styles.bar}>
			{clone ? <Clone workspace={workspace} clone={clone} /> : null}
			{missed.length > 0 ? <Missed workspace={workspace} failed={missed} /> : null}
		</View>
	);
};

const styles = StyleSheet.create({
	bar: { backgroundColor: color.background, paddingHorizontal: 10, paddingVertical: 8, gap: 8 },
	box: {
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: radius.lg,
		backgroundColor: color.card,
		padding: 12,
		gap: 6,
	},
	title: { color: color.foregroundStrong, fontSize: size.small, fontWeight: "600" },
	body: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	track: {
		height: 4,
		marginTop: 2,
		borderRadius: 2,
		overflow: "hidden",
		backgroundColor: color.track,
	},
	fill: { height: 4, borderRadius: 2, backgroundColor: color.connecting },
	unknown: { opacity: 0.5 },
	failed: { color: color.errorText, fontSize: size.meta },
	actions: { flexDirection: "row", justifyContent: "flex-end", gap: 8 },
	// About four files show, as on the desktop, and the rest scroll.
	list: { maxHeight: 112, flexGrow: 0 },
	missed: { gap: 1, paddingVertical: 3 },
	path: { color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
});
