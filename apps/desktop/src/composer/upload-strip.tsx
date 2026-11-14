// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The row along the top of the composer while files copy to a remote
// agent's device: how far they've got and how fast, with Cancel. It says
// when the copy waits for the link or for room, and offers Retry when a
// copy fails. Each chip shows its own file's progress below it.

import { CircleAlert, LoaderCircle, Pause, TriangleAlert } from "lucide-react";

import { useNow } from "@maxhq/shared/lib/clock";

import { type Chip, formatProgress, RATE_WINDOW_MS, type RateSample } from "./attachments";
import { formatRate, largeWarning, rateOf, uploadTotals } from "./upload";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const ROW =
	"flex h-[30px] items-center gap-2 border-b border-secondary px-3 text-[11.5px] text-muted-foreground";

const TEXT_BUTTON =
	"ml-auto shrink-0 rounded-[5px] px-1.5 py-0.5 text-[11.5px] text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

const files = (count: number) =>
	count === 1 ? translate("1 file") : translate("{{count}} files", { count: count });

/** What the strip shows about a message's uploads. */
export type UploadStripProps = {
	readonly chips: readonly Chip[];
	readonly samples: readonly RateSample[];
	/** The device's name, such as "devbox". */
	readonly device: string;
	/** Whether the device's link is down, which pauses every upload. */
	readonly linkDown: boolean;
	readonly onCancel: () => void;
	readonly onRetry: () => void;
};

/** The upload strip, or nothing while no file uploads or waits for Retry. */
export const UploadStrip = (props: UploadStripProps) => {
	useI18n();
	const totals = uploadTotals(props.chips);
	if (totals.active > 0) return <Uploading {...props} />;
	if (totals.retryable === 0) return null;
	return (
		<div role="group" aria-label={translate("Upload")} className={ROW}>
			<CircleAlert
				aria-hidden
				className="size-3 shrink-0 text-status-error-text"
				strokeWidth={2.2}
			/>
			<span className="truncate text-status-error-text">
				{translate("Couldn't upload {{files}} to {{device}}", {
					files: files(totals.retryable),
					device: props.device,
				})}
			</span>
			<button type="button" className={TEXT_BUTTON} onClick={props.onRetry}>
				{translate("Retry")}
			</button>
		</div>
	);
};

const Uploading = ({ chips, samples, device, linkDown, onCancel }: UploadStripProps) => {
	useI18n();
	const now = useNow();
	const totals = uploadTotals(chips);
	const warning = largeWarning(chips.filter((chip) => chip.state === "uploading"));
	const paused = linkDown || totals.paused;
	const rate = paused
		? null
		: rateOf(samples.filter((sample) => now - sample.at <= RATE_WINDOW_MS));
	const progress = totals.total > 0 ? formatProgress(totals.sent, totals.total) : null;
	const detail = [
		progress,
		rate === null ? null : translate("at {{rate}}", { rate: formatRate(rate) }),
	]
		.filter(Boolean)
		.join(" ");

	return (
		<div role="group" aria-label={translate("Upload")}>
			<div className={ROW}>
				{paused ? (
					<Pause aria-hidden className="size-3 shrink-0" strokeWidth={2.2} />
				) : (
					<LoaderCircle aria-hidden className="size-3 shrink-0 animate-spin" />
				)}
				<span className="truncate">
					{paused ? (
						<>
							<span className="text-foreground">
								{translate("Paused, reconnecting")}
							</span>
							{translate(" to {{device}}", { device: device })}
						</>
					) : totals.waiting ? (
						<>
							<span className="text-foreground">{translate("Waiting for room")}</span>
							{translate(" on {{device}}", { device: device })}
						</>
					) : (
						<>
							<span className="text-foreground">
								{translate("Uploading {{files}}", { files: files(totals.active) })}
							</span>
							{translate(" to {{device}}… ", { device: device })}
							<span className="text-foreground tabular-nums">{`${totals.percent}%`}</span>
						</>
					)}
				</span>
				{detail !== "" && <span className="shrink-0 tabular-nums">{detail}</span>}
				<button type="button" className={TEXT_BUTTON} onClick={onCancel}>
					{translate("Cancel upload")}
				</button>
			</div>
			{warning !== null && (
				<div className="flex h-[26px] items-center gap-2 border-b border-secondary px-3 text-[11.5px] text-status-busy-text">
					<TriangleAlert aria-hidden className="size-3 shrink-0" strokeWidth={2.2} />
					<span className="truncate">{warning}</span>
				</div>
			)}
		</div>
	);
};
