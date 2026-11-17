// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Over the composer while an agent's folder is still cloning, with how far
// git has got and Cancel, or once the clone failed or was cancelled, with
// Remove and Retry, as the desktop has it. Agents can't start in the folder
// until the clone is done.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import { failureOf, type SendState } from "@maxhq/shared/agent-view/answer";
import type { WorkspaceId } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { cloneOf, cloneWording } from "@maxhq/shared/workspaces/state";

import { useServices, useView } from "../../services";
import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";

export const CloneBanner = ({ workspace }: { workspace: WorkspaceId }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [state, setState] = useState<SendState>({ state: "idle" });
	const clone = useView((s) => {
		const found = s.view.tables.workspace.get(workspace);
		return found ? cloneOf(found) : null;
	});
	if (!clone) return null;
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
		<View style={styles.bar}>
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
								disabled={busy}
								onPress={() =>
									run(source.removeClone, t("Couldn't remove the folder."))
								}
							/>
							<Button
								label={t("Retry")}
								tone="primary"
								small
								disabled={busy}
								onPress={() =>
									run(source.retryClone, t("Couldn't retry the clone."))
								}
							/>
						</>
					)}
				</View>
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	bar: { backgroundColor: color.background, paddingHorizontal: 10, paddingVertical: 8 },
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
});
