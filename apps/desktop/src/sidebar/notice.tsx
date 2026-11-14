// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the sidebar shows under its list after an inbox change: what went
// wrong, until you dismiss it, and the notice with its Undo for five
// seconds, such as "Settled 3 agents".

import { useI18n } from "@maxhq/shared/i18n";

import { CircleAlert, X } from "lucide-react";

import { useServices } from "@/app/services";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { dismissFailures, dismissNotice, undoNotice } from "@maxhq/shared/ui/inbox-notice";

import { noticesOf, useNotices } from "./notices";

const DISMISS =
	"grid size-[18px] shrink-0 cursor-pointer place-items-center rounded-[5px] text-muted-foreground hover:bg-border hover:text-foreground";

/** The failures and the notice, each hidden when there's none. */
export const InboxNotice = () => {
	const { t } = useI18n();

	const services = useServices();
	const notice = useNotices((s) => s.notice);
	const failures = useNotices((s) => s.failures);
	const notices = noticesOf(services);
	return (
		<>
			{failures.length > 0 ? (
				<div className="mx-1 mt-1.5 flex shrink-0 items-start gap-2 rounded-[7px] border border-status-error/30 bg-status-error/8 py-1.5 pr-1 pl-2">
					<CircleAlert
						aria-hidden
						size={12}
						strokeWidth={2.2}
						className="mt-px shrink-0 text-status-error-text"
					/>
					<div role="alert" className="flex min-w-0 flex-1 flex-col gap-1">
						{failures.map((line) => (
							<p key={line} className="text-[11.5px] text-status-error-text">
								{line}
							</p>
						))}
					</div>
					<button
						type="button"
						aria-label={t("Dismiss")}
						onClick={() => dismissFailures(notices)}
						className={DISMISS}
					>
						<X aria-hidden size={11} strokeWidth={2.4} />
					</button>
				</div>
			) : null}
			{/* Always there, so a screen reader hears each new notice. */}
			<div role="status" className="flex shrink-0 flex-col">
				{notice ? (
					<div className="mx-1 mt-1.5 flex h-[30px] items-center gap-1 rounded-[7px] border border-track bg-background pr-1 pl-2 text-xs">
						<span
							title={notice.text}
							className="min-w-0 flex-1 truncate text-foreground"
						>
							{notice.text}
						</span>
						<button
							type="button"
							title={t("Undo {{value1}}", { value1: keyHint(SHORTCUTS.undo) })}
							onClick={() => undoNotice(notices)}
							className="h-[22px] shrink-0 cursor-pointer rounded-md px-1.5 font-semibold text-foreground-strong hover:bg-border"
						>
							{t("Undo")}
						</button>
						<button
							type="button"
							aria-label={t("Dismiss")}
							onClick={() => dismissNotice(notices)}
							className={DISMISS}
						>
							<X aria-hidden size={11} strokeWidth={2.4} />
						</button>
					</div>
				) : null}
			</div>
		</>
	);
};
