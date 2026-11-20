// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The button beside the version in the status bar. With no update known
// it checks now, and says how that went for a moment. With one on offer it
// asks to update, through the same prompt that shows the download. It
// adds no checks of its own beyond your click.

import { RotateCw } from "lucide-react";
import { useEffect, useState } from "react";

import { cn } from "@/lib/utils";
import { useI18n } from "@maxhq/shared/i18n";

import { checkNow, showPrompt, statusText, updates, useUpdates, versionButton } from "./store";

/** How long "Up to date" or "Couldn't check" stays after a click. */
export const NOTE_MS = 4000;

type Note = "up_to_date" | "failed" | null;

export const UpdateButton = () => {
	const { t } = useI18n();
	const state = useUpdates((s) => s);
	const button = versionButton(state);
	const status = state.update?.status;
	const { error } = state;
	const [note, setNote] = useState<Note>(null);
	useEffect(() => {
		if (note === null) return;
		const timer = setTimeout(() => setNote(null), NOTE_MS);
		return () => clearTimeout(timer);
	}, [note]);

	if (button === null) return null;
	if (button.kind === "update")
		return (
			<button
				type="button"
				className="h-[18px] rounded-full bg-primary px-2 text-[11px] font-[550] text-primary-foreground hover:bg-primary/90"
				title={t("Update to version {{version}}", { version: button.version })}
				onClick={showPrompt}
			>
				{t("Update")}
			</button>
		);

	const off = button.kind === "off";
	const checking = button.kind === "check" && button.checking;
	const failure =
		status?.state === "failed"
			? statusText(status)
			: error !== null
				? t("Couldn't check for updates: {{reason}}", { reason: error })
				: null;
	const tip = off
		? button.tip
		: note === "failed" && failure !== null
			? failure
			: t("Check for updates");
	const check = async () => {
		if (off || checking) return;
		setNote(null);
		await checkNow();
		const after = updates.getState();
		const state = after.update?.status.state;
		if (state === "up_to_date") setNote("up_to_date");
		else if (state === "failed" || after.error !== null) setNote("failed");
	};
	return (
		<span className="flex items-center gap-1.5" title={tip}>
			<button
				type="button"
				aria-label={t("Check for updates")}
				aria-disabled={off || checking}
				className={cn(
					"flex size-[18px] items-center justify-center rounded-[4px]",
					off ? "cursor-default opacity-40" : "hover:bg-accent hover:text-foreground"
				)}
				onClick={() => void check()}
			>
				<RotateCw className={cn("size-3", checking && "animate-spin")} aria-hidden />
			</button>
			{note !== null ? (
				<span
					role="status"
					className={note === "failed" ? "text-status-error-text" : undefined}
				>
					{note === "up_to_date" ? t("Up to date") : t("Couldn't check")}
				</span>
			) : null}
		</span>
	);
};
