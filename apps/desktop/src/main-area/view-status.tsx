// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the main area shows until the view loads: a spinner while it loads,
// and after a failure what went wrong, when it tries again on its own, and
// a Retry button that tries now.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Spinner } from "@/agent-view/icons";
import { useView } from "@/app/services";
import { SECONDARY } from "@/settings/controls";
import type { Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";

/** "Trying again in 5 s.", or "Trying again now." once the wait is up. */
export const retryNote = (retryAt: Timestamp | null, now: Timestamp): string =>
	retryAt === null || retryAt <= now
		? t("Trying again now.")
		: t("Trying again in {{value1}} s.", {
				value1: formatNumber(Math.ceil((retryAt - now) / 1000)),
			});

/** The main area while the view loads. */
export const ViewLoading = () => {
	const { t } = useI18n();
	return (
		<div className="grid h-full place-items-center px-8">
			<p role="status" className="flex items-center gap-2 text-[13px] text-muted-foreground">
				<Spinner size={12} />
				{t("Loading your agents")}
			</p>
		</div>
	);
};

/** The main area after the view failed to load, until a try works. */
export const ViewFailed = ({
	message,
	retryAt,
}: {
	message: string;
	retryAt: Timestamp | null;
}) => {
	const { t } = useI18n();

	const retry = useView((s) => s.retry);
	// Ticks only while this shows, for the countdown.
	const now = useNow(1000);
	return (
		<div className="grid h-full place-items-center px-8">
			<div className="flex max-w-[48ch] flex-col items-center gap-3 text-center text-[13px] text-muted-foreground">
				<p role="alert">
					{t("Couldn't load your agents. {{message}}", { message: message })}
				</p>
				<p>{retryNote(retryAt, now)}</p>
				<button
					type="button"
					className={SECONDARY}
					disabled={retryAt === null}
					onClick={retry}
				>
					{t("Retry")}
				</button>
			</div>
		</div>
	);
};
