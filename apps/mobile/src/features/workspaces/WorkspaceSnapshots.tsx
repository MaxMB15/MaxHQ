// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A container workspace's snapshots under its row: the list with Refresh,
// and for each one Restore as new, which changes nothing that's there, and
// Roll back, which asks first and says what unsaved work it would lose.
// Restoring needs the Mac's switch for this phone; listing doesn't.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { Timestamp, Workspace } from "@maxhq/shared/bindings";
import {
	refreshSnapshots,
	restoreSnapshot,
	snapshotLine,
	useSnapshots,
} from "@maxhq/shared/hosts/snapshots";
import { rollbackLoses } from "@maxhq/shared/hosts/unsaved";
import { sentence } from "@maxhq/shared/status/format";

import { useServices } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { confirm } from "../../ui/choose";
import { Buttons, Muted, type Note, NoteLine } from "../devices/section";
import { NeedsSwitch } from "../phone/everything";

export const WorkspaceSnapshots = ({
	workspace,
	instance,
	live,
	lets,
	now,
}: {
	workspace: Workspace;
	/** The container's name. */
	instance: string;
	/** Whether its device answers, so a restore can go. */
	live: boolean;
	/** Whether the Mac lets this phone restore. */
	lets: boolean;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const list = useSnapshots(source, workspace.id);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);

	const restore = async (snapshot: string, asNew: boolean, discardUnsaved: boolean) => {
		setBusy(true);
		setNote({ text: asNew ? t("Making a new workspace") : t("Rolling back"), error: false });
		try {
			await restoreSnapshot(source, workspace.id, snapshot, asNew, discardUnsaved);
			setNote({
				text: asNew
					? t("Made a new workspace from {{snapshot}}", { snapshot: snapshot })
					: t("Rolled back to {{snapshot}}", { snapshot: snapshot }),
				error: false,
			});
		} catch (error) {
			setNote({
				text: error instanceof Error ? sentence(error.message) : t("Couldn't restore it."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	const rollBack = (snapshot: string) => {
		const loses = rollbackLoses(workspace, false);
		const detail = [
			loses ? t("Rolling back loses this: {{loses}}.", { loses: loses }) : "",
			t(
				"{{instance}} stops, goes back to {{name}} and starts again. MaxHQ snapshots how it is now first.",
				{ instance: instance, name: snapshot }
			),
		]
			.filter(Boolean)
			.join(" ");
		confirm(
			{
				title: t("Roll back to {{name}}", { name: snapshot }),
				detail: loses
					? `${detail}${t(" Push your work first, or restore as new to keep both.")}`
					: detail,
			},
			loses ? t("Discard and roll back") : t("Roll back"),
			() => void restore(snapshot, false, loses !== null)
		);
	};

	const refresh = async () => {
		setNote(null);
		await refreshSnapshots(source, workspace.id);
	};

	const snapshots = list?.snapshots ?? [];
	const restores = live && lets && !busy;

	return (
		<View
			style={styles.box}
			accessibilityLabel={t("Snapshots of {{instance}}", { instance: instance })}
		>
			<View style={styles.head}>
				<Text style={styles.title}>{t("Snapshots")}</Text>
				<Button
					label={t("Refresh")}
					small
					disabled={busy || list?.state === "loading"}
					onPress={() => void refresh()}
				/>
			</View>
			{list?.state === "failed" ? (
				<Text accessibilityRole="alert" style={styles.error}>
					{sentence(list.error)}
				</Text>
			) : null}
			{list === undefined || (list.state === "loading" && list.snapshots === null) ? (
				<Muted>{t("Listing snapshots")}</Muted>
			) : snapshots.length === 0 ? (
				<Muted>{t("No snapshots yet.")}</Muted>
			) : (
				snapshots.map((snapshot) => (
					<View
						key={snapshot.name}
						style={styles.snapshot}
						accessibilityLabel={t("Snapshot {{name}}", { name: snapshot.name })}
					>
						<Text numberOfLines={1} style={styles.name}>
							{snapshot.name}
						</Text>
						<Text style={styles.line}>{snapshotLine(snapshot, now)}</Text>
						<Buttons>
							<Button
								label={t("Restore as new")}
								hint={t("Restore {{name}} as a new workspace", {
									name: snapshot.name,
								})}
								small
								disabled={!restores}
								onPress={() => void restore(snapshot.name, true, false)}
							/>
							<Button
								label={t("Roll back")}
								hint={t("Roll {{instance}} back to {{name}}", {
									instance: instance,
									name: snapshot.name,
								})}
								tone="danger"
								small
								disabled={!restores}
								onPress={() => rollBack(snapshot.name)}
							/>
						</Buttons>
					</View>
				))
			)}
			<NoteLine note={note} />
			{snapshots.length > 0 && !lets ? <NeedsSwitch /> : null}
		</View>
	);
};

const styles = StyleSheet.create({
	box: {
		gap: 8,
		padding: 12,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.subpanel,
	},
	head: { flexDirection: "row", alignItems: "center", justifyContent: "space-between" },
	title: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "600" },
	error: { color: color.errorText, fontSize: size.meta },
	snapshot: {
		gap: 6,
		paddingTop: 8,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	name: { color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	line: { color: color.mutedForeground, fontSize: size.tiny },
});
