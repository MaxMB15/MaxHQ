// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One container workspace MaxHQ made, as a row of its device's card: its
// dot and state, a bar while it's being made, the guest and egress lines,
// its unsaved work, Start, Stop and Delete, and its snapshots. Delete asks
// first, and when the device refuses because of unpushed work it says so
// and asks again before throwing that work away. Deleting needs the Mac's
// switch for this phone; starting and stopping don't.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { ActionResult } from "@maxhq/shared/data/source";
import { unsavedOf } from "@maxhq/shared/hosts/unsaved";
import { useNow } from "@maxhq/shared/lib/clock";
import { sentence } from "@maxhq/shared/status/format";
import { bootLine, deleteRefusal } from "@maxhq/shared/workspaces/device";
import {
	ACTION_LABEL,
	ACTION_NOTE,
	cloneLabel,
	cloneOf,
	egressText,
	guestText,
	type MadeWorkspace,
	stateLook,
	stateText,
	type WorkspaceAction,
	workspaceActions,
} from "@maxhq/shared/workspaces/state";

import { useServices } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { confirm } from "../../ui/choose";
import { ConnectionDot } from "../../ui/Dot";
import { Buttons, CardBody, Muted, NoteLine, useAction } from "../devices/section";
import { LOOK_TEXT, madeShare } from "./look";
import { WorkspaceSnapshots } from "./WorkspaceSnapshots";

export const WorkspaceRow = memo(function WorkspaceRow({
	workspace,
	project,
	where,
	live,
	lets,
	snapshots,
}: {
	workspace: MadeWorkspace;
	/** The project's name, or null for none. */
	project: string | null;
	/** The device's name. */
	where: string;
	/** Whether the device answers. */
	live: boolean;
	/** Whether the Mac lets this phone delete it. */
	lets: boolean;
	/** Whether the device offers its snapshots now. */
	snapshots: boolean;
}) {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(5000);
	const { busy, note, setNote, run } = useAction();
	const [showSnapshots, setShowSnapshots] = useState(false);

	const { managed, location } = workspace;
	const look = stateLook(managed.state);
	const state = cloneLabel(cloneOf(workspace)) ?? stateText(managed.state);
	const actions = live ? workspaceActions(managed.state) : [];
	const unsaved = unsavedOf(workspace, now);

	const remove = async (discard: boolean) => {
		const seen: { refused: string | null } = { refused: null };
		await run(ACTION_NOTE.delete, async (): Promise<ActionResult> => {
			const result = await source.deleteWorkspace(workspace.id, discard);
			seen.refused = deleteRefusal(result);
			return result;
		});
		if (seen.refused === null) return;
		setNote(null);
		confirm(
			{
				title: `${sentence(seen.refused)}.`,
				detail: t("Push it first to keep it. Deleting now throws that work away for good."),
			},
			t("Discard and delete"),
			() => void remove(true)
		);
	};

	const press = (action: WorkspaceAction) => {
		switch (action) {
			case "start":
				return void run(ACTION_NOTE.start, () => source.startWorkspace(workspace.id));
			case "stop":
				return void run(ACTION_NOTE.stop, () => source.stopWorkspace(workspace.id));
			case "delete":
				return confirm(
					{
						title: t("Delete {{instance}}", { instance: location.instance }),
						detail: t("The container and everything in it go, and so do its agents."),
					},
					t("Delete"),
					() => void remove(false)
				);
		}
	};

	return (
		<CardBody>
			<View
				style={styles.top}
				accessible
				accessibilityLabel={`${t("Workspace {{instance}}", { instance: location.instance })}, ${state}`}
			>
				<ConnectionDot look={look} size={8} surface={color.card} decorative />
				<Text numberOfLines={1} style={styles.instance}>
					{location.instance}
				</Text>
				{project ? (
					<Text numberOfLines={1} style={styles.project}>
						{project}
					</Text>
				) : null}
			</View>
			<Text style={[styles.state, { color: LOOK_TEXT[look] }]}>{state}</Text>

			{managed.state.state === "creating" ? (
				<View
					accessibilityRole="progressbar"
					accessibilityLabel={t("Making {{instance}}", { instance: location.instance })}
					accessibilityValue={{
						min: 0,
						max: managed.state.of,
						now: managed.state.done,
					}}
					style={styles.track}
				>
					<View
						style={[
							styles.fill,
							{ width: `${madeShare(managed.state.done, managed.state.of) * 100}%` },
						]}
					/>
				</View>
			) : null}

			<View style={styles.lines}>
				{managed.state.state === "creating" ? null : (
					<Text style={styles.line}>{guestText(managed.guest, now)}</Text>
				)}
				<Text style={[styles.line, managed.egress.state === "failed" && styles.error]}>
					{egressText(managed.egress, now)}
				</Text>
				<Text style={styles.line}>{bootLine(workspace, where)}</Text>
				{unsaved ? (
					<Text style={[styles.line, unsaved.any && !unsaved.old && styles.unsaved]}>
						{`${unsaved.text}. ${unsaved.readAt}.`}
					</Text>
				) : null}
				<Text numberOfLines={1} style={[styles.line, styles.path]}>
					{location.path}
				</Text>
			</View>

			{actions.length > 0 || snapshots ? (
				<Buttons>
					{actions.map((action) => (
						<Button
							key={action}
							label={ACTION_LABEL[action]}
							tone={action === "delete" ? "danger" : "secondary"}
							small
							disabled={busy || (action === "delete" && !lets)}
							onPress={() => press(action)}
						/>
					))}
					{snapshots ? (
						<Button
							label={showSnapshots ? t("Hide snapshots") : t("Snapshots")}
							small
							onPress={() => setShowSnapshots((on) => !on)}
						/>
					) : null}
				</Buttons>
			) : null}
			<NoteLine note={note} />

			{snapshots && showSnapshots ? (
				<WorkspaceSnapshots
					workspace={workspace}
					instance={location.instance}
					live={live}
					lets={lets}
					now={now}
				/>
			) : null}

			{!live && workspaceActions(managed.state).length > 0 ? (
				<Muted>
					{t("Connect to {{where}} to start, stop or delete it.", { where: where })}
				</Muted>
			) : null}
		</CardBody>
	);
});

const styles = StyleSheet.create({
	top: { flexDirection: "row", alignItems: "center", gap: 8 },
	instance: {
		flexShrink: 1,
		color: color.foregroundStrong,
		fontFamily: font.mono,
		fontSize: size.small,
	},
	project: { flexShrink: 1, color: color.mutedForeground, fontSize: size.meta },
	state: { fontSize: size.meta, paddingLeft: 16 },
	track: {
		height: 4,
		marginLeft: 16,
		borderRadius: 2,
		overflow: "hidden",
		backgroundColor: color.track,
	},
	fill: { height: 4, borderRadius: 2, backgroundColor: color.connecting },
	lines: { gap: 2, paddingLeft: 16 },
	line: { color: color.mutedForeground, fontSize: size.meta },
	error: { color: color.errorText },
	unsaved: { color: color.secondaryForeground },
	path: { fontFamily: font.mono, fontSize: size.tiny },
});
