// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An output pill's preview: which command, how it stands, and exactly the
// text the agent reads, masked the way it goes.

import { SquareTerminal } from "lucide-react";

import { t, useI18n } from "@maxhq/shared/i18n";
import type { OutputRef } from "@maxhq/shared/pills/draft";
import { OUTPUT_LINES } from "@maxhq/shared/terminal/output";

import { withParts } from "@/lib/parts";
import { cn } from "@/lib/utils";
import { useMasked } from "@/native/mask";

import { outputFailed, outputState } from "./registry";

/** The facts after the command's name: how it stands, its length and its device. */
const factsOf = (output: OutputRef): string =>
	[
		outputState(output),
		output.lines ? null : t("{{count, number}} line", { count: output.lines_total }),
		output.device_name || null,
	]
		.filter(Boolean)
		.join(" · ");

/**
 * What goes along with the pill. `editable` adds how to keep it back, for a
 * pill still in the composer.
 */
export const OutputPreview = ({ output, editable }: { output: OutputRef; editable: boolean }) => {
	useI18n();
	const masked = useMasked(output.text);
	const failed = outputFailed(output);
	return (
		<div className="flex w-[min(380px,calc(100vw-48px))] flex-col" data-output-preview="">
			<div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs">
				<SquareTerminal
					aria-hidden
					className={cn(
						"size-3.5 shrink-0",
						failed ? "text-(--pill-output-failed-text)" : "text-muted-foreground"
					)}
				/>
				<span className="min-w-0 font-medium break-all">
					{withParts("Output of {{from}}", {
						from: <span className="font-mono">{output.command.trim()}</span>,
					})}
				</span>
				<span className="text-muted-foreground">{factsOf(output)}</span>
			</div>
			<pre
				className={cn(
					"mt-2 mb-1.5 max-h-56 overflow-auto rounded-md border border-border px-[9px] py-1.5",
					"font-mono text-[11.5px] leading-normal whitespace-pre-wrap break-all text-foreground"
				)}
			>
				{masked === null ? "" : masked || t("No output yet")}
			</pre>
			{editable && (
				<p className="text-[11.5px] text-muted-foreground">
					{t(
						"The agent gets the last {{lines, number}} lines. MaxHQ masks keys and tokens it recognizes. Backspace the pill to keep it private.",
						{ lines: OUTPUT_LINES }
					)}
				</p>
			)}
		</div>
	);
};
