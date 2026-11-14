// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The question that offers a newer version: which one, a link to what's
// new in it, Later and Update now. Update now shows the download's
// progress here until the app restarts into the new version. Later hides
// it until MaxHQ starts again, and Settings still shows the update. One
// lives with each window.

import { AlertDialog } from "@base-ui/react/alert-dialog";

import { OutLink } from "@/components/out-link";
import { cn } from "@/lib/utils";
import { PRIMARY, SECONDARY } from "@/settings/controls";
import { BACKDROP, POPUP } from "@/workspaces/new-workspace";
import { useI18n } from "@maxhq/shared/i18n";

import {
	installing,
	later,
	progressOf,
	promptVersion,
	releaseNotesUrl,
	statusText,
	updateNow,
	useUpdates,
} from "./store";

/** The bar that fills as the download comes in, or moves while its size isn't known. */
const ProgressBar = ({ value }: { value: number | null }) => {
	const { t } = useI18n();
	return (
		<div
			role="progressbar"
			aria-label={t("Update progress")}
			aria-valuemin={0}
			aria-valuemax={100}
			aria-valuenow={value === null ? undefined : Math.round(value * 100)}
			className="h-1.5 overflow-hidden rounded-full bg-track"
		>
			<div
				className={cn(
					"h-full rounded-full bg-primary transition-[width]",
					value === null && "w-1/3 animate-pulse"
				)}
				style={value === null ? undefined : { width: `${value * 100}%` }}
			/>
		</div>
	);
};

/** The update prompt, shown while a newer version waits for an answer. */
export const UpdatePrompt = () => {
	const { t } = useI18n();
	const version = useUpdates(promptVersion);
	const status = useUpdates((s) => s.update?.status);
	const error = useUpdates((s) => s.error);
	const releases = useUpdates((s) => s.update?.release_notes);
	const notes = version === null ? null : releaseNotesUrl(releases, version);
	const busy = installing(status);
	const failed = status?.state === "failed";
	return (
		<AlertDialog.Root
			open={version !== null}
			onOpenChange={(open) => {
				if (!open && version !== null && !busy) later(version);
			}}
		>
			<AlertDialog.Portal>
				<AlertDialog.Backdrop className={BACKDROP} />
				<AlertDialog.Popup className={cn(POPUP, "w-[min(420px,calc(100vw-32px))]")}>
					{version !== null && status ? (
						<div className="flex flex-col gap-4">
							<div className="flex flex-col gap-1">
								<AlertDialog.Title className="text-[15px] font-[650] text-foreground-strong">
									{t("MaxHQ {{version}} is available", { version })}
								</AlertDialog.Title>
								<AlertDialog.Description
									className={cn(
										"text-[12.5px]",
										failed ? "text-status-error-text" : "text-muted-foreground"
									)}
								>
									{busy || failed
										? statusText(status)
										: t("MaxHQ downloads it, puts it in place and restarts.")}
								</AlertDialog.Description>
								{error && !failed ? (
									<span
										role="status"
										className="text-[11.5px] text-status-error-text"
									>
										{error}
									</span>
								) : null}
							</div>
							{busy ? <ProgressBar value={progressOf(status)} /> : null}
							<div className="flex items-center justify-between gap-2">
								{notes ? (
									<OutLink href={notes} className="text-[12.5px]">
										{t("What's new")}
									</OutLink>
								) : (
									<span />
								)}
								<div className="flex items-center gap-2">
									<AlertDialog.Close className={SECONDARY} disabled={busy}>
										{t("Later")}
									</AlertDialog.Close>
									<button
										type="button"
										className={PRIMARY}
										disabled={busy}
										onClick={() => void updateNow()}
									>
										{failed ? t("Try again") : t("Update now")}
									</button>
								</div>
							</div>
						</div>
					) : null}
				</AlertDialog.Popup>
			</AlertDialog.Portal>
		</AlertDialog.Root>
	);
};
