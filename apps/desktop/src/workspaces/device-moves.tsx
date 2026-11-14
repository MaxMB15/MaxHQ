// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The moves of a device's old containers into ones MaxHQ runs, a card for
// each. Every step says what it changes and waits for you. The cards don't
// wait on each other, so you take them in whatever order suits. Containers
// MaxHQ didn't make and isn't moving get a Move into MaxHQ button.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import type { DeviceId, Move, Timestamp } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
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
	type StepStatus,
} from "@maxhq/shared/hosts/moves";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { DANGER, PRIMARY, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import { sentence } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";

const STATUS_LABEL: Record<StepStatus, string> = {
	get done() {
		return t("Done");
	},
	get ready() {
		return t("Waiting for you");
	},
	get running() {
		return t("Running");
	},
	get failed() {
		return t("Failed");
	},
	get todo() {
		return t("Not yet");
	},
};

const STATUS_TONE: Record<StepStatus, string> = {
	done: "bg-status-ready",
	ready: "bg-primary",
	running: "bg-status-connecting animate-pulse",
	failed: "bg-status-error",
	todo: "border-[1.5px] border-dashed border-status-off",
};

/** Sends one call and puts the device's answer in the note. */
const useCall = () => {
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const send = async (call: () => Promise<ActionResult>) => {
		setBusy(true);
		setNote({ text: t("Sending"), error: false });
		try {
			const failure = failureOf(await call());
			setNote(failure ? { text: sentence(failure), error: true } : null);
		} catch (error) {
			console.warn("Couldn't reach the device for a move", error);
			setNote({ text: t("Couldn't reach the device. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return { busy, note, send };
};

const MoveCard = ({
	move,
	view,
	live,
	now,
}: {
	move: Move;
	view: ViewState;
	live: boolean;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, send } = useCall();
	const [retiring, setRetiring] = useState(false);
	const steps = moveSteps(move);
	const actions = moveActions(move);
	const line = moveNote(move, now);

	const press = (action: MoveAction) => {
		switch (action.kind) {
			case "run":
				return void send(() => source.runMoveStep(move.id, action.step));
			case "move_back":
				return void send(() => source.moveBack(move.id));
			case "retire":
				return setRetiring(true);
		}
	};

	return (
		<div
			role="group"
			aria-label={t("Move of {{source}}", { source: move.source })}
			className="flex flex-col gap-2 border-t border-sidebar-border px-4 py-3 first:border-t-0"
		>
			<div className="flex items-center gap-2.5">
				<span className="min-w-0 truncate font-mono text-[12.5px] text-foreground-strong">
					{move.source}
				</span>
				<span className="text-[11.5px] text-muted-foreground">{t("to")}</span>
				<span className="min-w-0 truncate font-mono text-[12.5px] text-foreground">
					{move.target}
				</span>
				<span className="flex-1" />
				<StatusNote note={note} />
				{!retiring
					? actions.map((action) => (
							<button
								key={`${action.kind}-${action.kind === "run" ? action.step : ""}`}
								type="button"
								className={action.kind === "retire" ? SECONDARY : PRIMARY}
								disabled={!live || busy}
								onClick={() => press(action)}
							>
								{action.label}
							</button>
						))
					: null}
			</div>

			{line ? <p className="text-xs text-secondary-foreground">{line}</p> : null}

			<ol className={cn("flex flex-col gap-1.5", isOver(move) && "opacity-70")}>
				{steps.map((step) => (
					<li
						key={step.step}
						aria-label={`${step.label}: ${STATUS_LABEL[step.status]}`}
						className="flex gap-2.5"
					>
						<span
							aria-hidden
							className={cn(
								"mt-[3px] size-2.5 shrink-0 rounded-full",
								STATUS_TONE[step.status]
							)}
						/>
						<div className="flex min-w-0 flex-1 flex-col gap-0.5">
							<span
								className={cn(
									"text-xs",
									step.status === "todo"
										? "text-muted-foreground"
										: "text-foreground"
								)}
							>
								{step.label}
								<span className="text-muted-foreground">
									{` · ${STATUS_LABEL[step.status]}`}
								</span>
							</span>
							{step.note ? (
								<span
									className={cn(
										"text-[11.5px]",
										step.status === "failed"
											? "text-status-error-text"
											: "text-secondary-foreground"
									)}
								>
									{step.note}
								</span>
							) : null}
							{step.changes.length > 0 && step.status !== "done" ? (
								<ul className="list-disc pl-4 text-[11.5px] text-muted-foreground">
									{step.changes.map((change) => (
										<li key={change}>{change}</li>
									))}
								</ul>
							) : null}
						</div>
					</li>
				))}
			</ol>

			{!live && actions.length > 0 ? (
				<p className="text-xs text-muted-foreground">
					{t("Connect to the device to go on.")}
				</p>
			) : null}

			{retiring ? (
				<div
					role="alert"
					aria-label={t("Retire {{source}}", { source: move.source })}
					className="flex flex-col gap-2 rounded-lg border border-status-error/45 bg-status-error/10 px-3 py-2.5"
				>
					<span className="text-xs font-[600] text-status-error-text">
						{t("Delete {{source}} and its move snapshot for good?", {
							source: move.source,
						})}
					</span>
					<span className="text-xs text-secondary-foreground">
						{retireLoses(view, move, now)}
					</span>
					<div className="flex items-center justify-end gap-2">
						<button
							type="button"
							className={SECONDARY}
							onClick={() => setRetiring(false)}
						>
							{t("Keep it")}
						</button>
						<button
							type="button"
							className={DANGER}
							disabled={!live || busy}
							onClick={() => {
								setRetiring(false);
								void send(() => source.retireMove(move.id));
							}}
						>
							{t("Delete {{source}}", { source: move.source })}
						</button>
					</div>
				</div>
			) : null}
		</div>
	);
};

const MovableRow = ({
	device,
	instance,
	live,
}: {
	device: DeviceId;
	instance: string;
	live: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, send } = useCall();
	return (
		<Row
			title={<span className="font-mono">{instance}</span>}
			sub={t(
				"Made outside MaxHQ. Moving copies it first, and the original keeps running until you switch."
			)}
		>
			<StatusNote note={note} />
			<button
				type="button"
				className={SECONDARY}
				aria-label={t("Move {{instance}} into MaxHQ", { instance: instance })}
				disabled={!live || busy}
				onClick={() => void send(() => source.planMove(device, instance))}
			>
				{t("Move into MaxHQ")}
			</button>
		</Row>
	);
};

/** A device's moves into MaxHQ, a card each, and the containers it could move. */
export const DeviceMoves = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
}) => {
	const { t } = useI18n();

	const now = useNow(5000);
	if (!offersMoves(view, device)) return null;
	const moves = movesOn(view, device);
	const movable = movableContainers(view, device);
	if (moves.length === 0 && movable.length === 0) return null;
	return (
		<Section
			title={t("Moves into MaxHQ")}
			label={t("Moves into MaxHQ on {{name}}", { name: name })}
			hint={t("Each step waits for you, in any order across cards")}
		>
			{moves.map((move) => (
				<MoveCard key={move.id} move={move} view={view} live={live} now={now} />
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
		</Section>
	);
};
