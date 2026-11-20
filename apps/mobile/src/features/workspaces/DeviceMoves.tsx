// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The moves of a device's old containers into ones MaxHQ runs, as the
// desktop shows them: a row for each move with its steps, each saying what
// it changes and waiting for you, and a Move into MaxHQ button for each
// container MaxHQ didn't make and isn't moving. Running a step needs the
// Mac's switch for this phone; planning, moving back and retiring don't.

import { useI18n } from "@maxhq/shared/i18n";

import { memo } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId, Move, Timestamp } from "@maxhq/shared/bindings";
import {
	isOver,
	movableContainers,
	type MoveAction,
	moveActions,
	moveNote,
	movesOn,
	moveSteps,
	offersMoves,
	retireLoses,
	STEP_STATUS_LABEL,
	type StepView,
} from "@maxhq/shared/hosts/moves";
import { useNow } from "@maxhq/shared/lib/clock";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useServices } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import {
	Buttons,
	CardBody,
	Muted,
	NoteLine,
	type SectionProps,
	useAction,
} from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { STEP_DOT } from "./look";

const Step = ({ step }: { step: StepView }) => {
	const dot = STEP_DOT[step.status];
	const status = STEP_STATUS_LABEL[step.status];
	return (
		<View style={styles.step} accessible accessibilityLabel={`${step.label}: ${status}`}>
			<View
				style={[
					styles.dot,
					dot.dashed
						? { borderColor: dot.color, borderWidth: 1.5, borderStyle: "dashed" }
						: { backgroundColor: dot.color },
				]}
			/>
			<View style={styles.stepText}>
				<Text style={[styles.stepLabel, step.status === "todo" && styles.todo]}>
					{step.label}
					<Text style={styles.todo}>{` · ${status}`}</Text>
				</Text>
				{step.note ? (
					<Text style={[styles.stepNote, step.status === "failed" && styles.error]}>
						{step.note}
					</Text>
				) : null}
				{step.changes.length > 0 && step.status !== "done"
					? step.changes.map((change) => (
							<Text key={change} style={styles.change}>
								{`• ${change}`}
							</Text>
						))
					: null}
			</View>
		</View>
	);
};

const MoveRow = memo(function MoveRow({
	move,
	view,
	live,
	lets,
	now,
}: {
	move: Move;
	view: ViewState;
	live: boolean;
	lets: boolean;
	now: Timestamp;
}) {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	const actions = moveActions(move);
	const line = moveNote(move, now);
	const steps = moveSteps(move);

	const press = (action: MoveAction) => {
		switch (action.kind) {
			case "run":
				return void run(t("Sending"), () => source.runMoveStep(move.id, action.step));
			case "move_back":
				return void run(t("Sending"), () => source.moveBack(move.id));
			case "retire":
				return confirm(
					{
						title: t("Delete {{source}} and its move snapshot for good?", {
							source: move.source,
						}),
						detail: retireLoses(view, move, now),
					},
					t("Delete {{source}}", { source: move.source }),
					() => void run(t("Sending"), () => source.retireMove(move.id))
				);
		}
	};
	const runs = actions.some((action) => action.kind === "run");

	return (
		<CardBody>
			<View
				style={styles.top}
				accessible
				accessibilityLabel={t("Move of {{source}}", { source: move.source })}
			>
				<Text numberOfLines={1} style={styles.source}>
					{move.source}
				</Text>
				<Text style={styles.to}>{t("to")}</Text>
				<Text numberOfLines={1} style={styles.target}>
					{move.target}
				</Text>
			</View>
			{line ? <Text style={styles.line}>{line}</Text> : null}
			<View style={[styles.steps, isOver(move) && styles.over]}>
				{steps.map((step) => (
					<Step key={step.step} step={step} />
				))}
			</View>
			{actions.length > 0 ? (
				<Buttons>
					{actions.map((action) => (
						<Button
							key={`${action.kind}-${action.kind === "run" ? action.step : ""}`}
							label={action.label}
							tone={action.kind === "retire" ? "danger" : "primary"}
							small
							disabled={!live || busy || (action.kind === "run" && !lets)}
							onPress={() => press(action)}
						/>
					))}
				</Buttons>
			) : null}
			<NoteLine note={note} />
			{!live && actions.length > 0 ? (
				<Muted>{t("Connect to the device to go on.")}</Muted>
			) : null}
			{runs && !lets ? <NeedsSwitch /> : null}
		</CardBody>
	);
});

const MovableRow = memo(function MovableRow({
	device,
	instance,
	live,
}: {
	device: DeviceId;
	instance: string;
	live: boolean;
}) {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	return (
		<CardBody>
			<Text numberOfLines={1} style={styles.source}>
				{instance}
			</Text>
			<Muted>
				{t(
					"Made outside MaxHQ. Moving copies it first, and the original keeps running until you switch."
				)}
			</Muted>
			<Buttons>
				<Button
					label={t("Move into MaxHQ")}
					hint={t("Move {{instance}} into MaxHQ", { instance: instance })}
					small
					disabled={!live || busy}
					onPress={() => void run(t("Sending"), () => source.planMove(device, instance))}
				/>
			</Buttons>
			<NoteLine note={note} />
		</CardBody>
	);
});

export const DeviceMoves = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const now = useNow(5000);
	const lets = useLetsPhone(device);
	if (!offersMoves(view, device)) return null;
	const moves = movesOn(view, device);
	const movable = movableContainers(view, device);
	if (moves.length === 0 && movable.length === 0) return null;
	return (
		<Card
			title={t("Moves into MaxHQ")}
			note={t("Each step waits for you, in any order across cards")}
			label={t("Moves into MaxHQ on {{name}}", { name: name })}
		>
			{moves.map((move) => (
				<MoveRow key={move.id} move={move} view={view} live={live} lets={lets} now={now} />
			))}
			{movable.map((w) =>
				w.location.type === "container" ? (
					<MovableRow
						key={w.id}
						device={device}
						instance={w.location.instance}
						live={live}
					/>
				) : null
			)}
		</Card>
	);
};

const styles = StyleSheet.create({
	top: { flexDirection: "row", alignItems: "center", gap: 8 },
	source: {
		flexShrink: 1,
		color: color.foregroundStrong,
		fontFamily: font.mono,
		fontSize: size.small,
	},
	to: { color: color.mutedForeground, fontSize: size.meta },
	target: { flexShrink: 1, color: color.foreground, fontFamily: font.mono, fontSize: size.small },
	line: { color: color.secondaryForeground, fontSize: size.meta },
	steps: { gap: 8 },
	over: { opacity: 0.7 },
	step: { flexDirection: "row", gap: 10 },
	dot: { width: 10, height: 10, borderRadius: 5, marginTop: 3 },
	stepText: { flex: 1, gap: 2 },
	stepLabel: { color: color.foreground, fontSize: size.meta },
	todo: { color: color.mutedForeground },
	stepNote: { color: color.secondaryForeground, fontSize: size.tiny },
	error: { color: color.errorText },
	change: { color: color.mutedForeground, fontSize: size.tiny, paddingLeft: 4 },
});
