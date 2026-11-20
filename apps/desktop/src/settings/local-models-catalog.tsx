// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Models card of the Local models page: the catalog a device can run,
// best pick first. Each model shows its file's quantization and size, how
// well it fits the device's GPU and how fast it should run there, its
// licence and published scores, and its notes behind a disclosure. Its
// buttons download, pause, resume, load, unload or delete the file through
// the device's `maxd`, and Delete asks first, naming the space it frees.

import { useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { Badge } from "@/tasks/activity";
import type { DeviceId, ModelAction, ModelScore } from "@maxhq/shared/bindings";
import { formatNumber, t as translate, useI18n } from "@maxhq/shared/i18n";
import { localModelsOf } from "@maxhq/shared/local-models/models";
import { FIT_TONE } from "@maxhq/shared/local-models/picker";
import { type CatalogRow, catalogRows } from "@maxhq/shared/local-models/select";
import {
	deleteQuestion,
	diskFreeText,
	modelRow,
	type RowButton,
	rowButtonLabel,
	rowModelAction,
} from "@maxhq/shared/local-models/settings-page";
import {
	coldTurnText,
	estimateNote,
	fitDetail,
	fitLabel,
	formatFileSize,
	scoreSourceText,
	speedText,
} from "@maxhq/shared/local-models/wording";
import type { ViewState } from "@maxhq/shared/view/replica";

import { ConfirmButton, PRIMARY, SECONDARY, Section, StatusNote } from "./controls";
import { failureOf, type Note } from "./result";

/**
 * Sends model actions to a device's `maxd`, one at a time, with a note that
 * says why one failed.
 */
export const useModelAction = (device: DeviceId) => {
	const { source } = useServices();
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const send = async (action: ModelAction): Promise<boolean> => {
		if (busy) return false;
		setBusy(true);
		setNote(null);
		try {
			const failure = failureOf(await source.modelAction(device, action));
			if (failure) setNote({ text: failure, error: true });
			return failure === null;
		} catch (error) {
			console.warn(translate("Couldn't change the local models"), error);
			setNote({ text: translate("Couldn't do it. Try again."), error: true });
			return false;
		} finally {
			setBusy(false);
		}
	};
	return { busy, note, send };
};

/** A score as the row shows it, such as "SWE-bench Verified 62.4%". */
const scoreText = (score: ModelScore): string =>
	translate("{{benchmark}} {{value}}%", {
		benchmark: score.benchmark,
		value: formatNumber(score.value ?? 0, { maximumFractionDigits: 1 }),
	});

/** A score with who ran it and who reported it, for the notes. */
const scoreLine = (score: ModelScore): string =>
	score.harness
		? translate("{{score}} in {{harness}}. {{source}}.", {
				score: scoreText(score),
				harness: score.harness,
				source: scoreSourceText(score),
			})
		: translate("{{score}}. {{source}}.", {
				score: scoreText(score),
				source: scoreSourceText(score),
			});

const ModelRow = ({
	device,
	view,
	row,
	disabled,
}: {
	device: DeviceId;
	view: ViewState;
	row: CatalogRow;
	/** Why the buttons can't act now, or null when they can. */
	disabled: string | null;
}) => {
	useI18n();
	const { busy, note, send } = useModelAction(device);
	const shown = modelRow(view, device, row);
	const estimate = row.estimate;
	const scores = row.model.scores.filter((s) => s.value !== null);
	const act = (button: RowButton) => {
		const action = rowModelAction(row, shown, button);
		if (action) void send(action);
	};
	const off = disabled !== null || busy || shown.busy;

	return (
		<div
			role="group"
			aria-label={row.model.name}
			className="flex flex-col gap-1.5 border-t border-sidebar-border px-4 py-3 first:border-t-0"
		>
			<div className="flex items-center gap-2">
				<span className="min-w-0 truncate text-[12.5px] font-[600] text-foreground">
					{row.model.name}
				</span>
				{row.isDefault ? <Badge tone="info">{translate("Best pick")}</Badge> : null}
				{estimate ? (
					<span title={fitDetail(estimate.fit)} className="inline-flex">
						<Badge tone={FIT_TONE[estimate.fit]}>{fitLabel(estimate.fit)}</Badge>
					</span>
				) : null}
				<span className="ml-auto flex shrink-0 items-center gap-2">
					<StatusNote note={note} />
					{shown.buttons.map((button) =>
						button === "delete" ? (
							<ConfirmButton
								key={button}
								label={rowButtonLabel(button)}
								question={deleteQuestion(row)}
								confirm={rowButtonLabel(button)}
								onConfirm={() => act(button)}
								disabled={off}
							/>
						) : (
							<button
								key={button}
								type="button"
								className={button === "download" ? PRIMARY : SECONDARY}
								disabled={off}
								onClick={() => act(button)}
							>
								{rowButtonLabel(button)}
							</button>
						)
					)}
				</span>
			</div>
			<div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
				<span>
					{translate("{{quant}}, {{size}}", {
						quant: row.file.quant,
						size: formatFileSize(row.file.bytes),
					})}
				</span>
				{estimate?.decode ? <span>{speedText(estimate.decode)}</span> : null}
				{estimate?.coldTurn ? (
					<span title={estimateNote()}>{coldTurnText(estimate.coldTurn)}</span>
				) : null}
				<span>{row.model.license}</span>
				{scores.slice(0, 1).map((score) => (
					<span key={score.benchmark} title={scoreSourceText(score)}>
						{scoreText(score)}
					</span>
				))}
			</div>
			{shown.status || shown.disk ? (
				<div className="flex items-center gap-2 text-xs">
					{shown.percent !== null ? (
						<span
							role="progressbar"
							aria-label={translate("Download of {{model}}", {
								model: row.model.name,
							})}
							aria-valuemin={0}
							aria-valuemax={100}
							aria-valuenow={Math.floor(shown.percent)}
							className="h-1.5 w-28 shrink-0 overflow-hidden rounded-full bg-track"
						>
							<span
								className="block h-full rounded-full bg-primary"
								style={{ width: `${shown.percent}%` }}
							/>
						</span>
					) : null}
					<span
						className={cn(
							shown.error || shown.disk?.short
								? "text-status-error-text"
								: "text-muted-foreground"
						)}
					>
						{shown.status ?? shown.disk?.text}
					</span>
				</div>
			) : null}
			{row.model.notes || scores.length > 0 ? (
				<details className="text-xs text-muted-foreground">
					<summary className="w-fit cursor-pointer text-secondary-foreground">
						{translate("Notes")}
					</summary>
					<div className="mt-1 flex flex-col gap-1">
						{row.model.notes ? <p>{row.model.notes}</p> : null}
						{scores.map((score) => (
							<p key={score.benchmark}>{scoreLine(score)}</p>
						))}
					</div>
				</details>
			) : null}
		</div>
	);
};

/**
 * The Models card: the catalog models a device can run, with what each
 * row can do with its file. Nothing for a device that runs no models itself.
 */
export const ModelsSection = ({
	device,
	view,
	disabled,
}: {
	device: DeviceId;
	view: ViewState;
	/** Why the buttons can't act now, or null when they can. */
	disabled: string | null;
}) => {
	useI18n();
	const rows = catalogRows(view, device);
	const local = localModelsOf(view, device);
	if (rows.length === 0) return null;
	return (
		<Section title={translate("Models")} hint={local ? diskFreeText(local) : undefined}>
			{disabled ? (
				<p className="px-4 py-2.5 text-xs text-muted-foreground">{disabled}</p>
			) : null}
			{rows.map((row) => (
				<ModelRow
					key={row.model.id}
					device={device}
					view={view}
					row={row}
					disabled={disabled}
				/>
			))}
		</Section>
	);
};
