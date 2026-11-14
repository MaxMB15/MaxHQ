// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sidebar's admission cards: one row for each program a device asks
// you about, oldest first, which opens that device's page, where you
// answer. Hidden when none wait.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { ShieldAlert } from "lucide-react";
import { useMemo } from "react";

import { useUi, useView } from "@/app/services";
import { admissionCard, admissionLink, openAdmissions } from "@maxhq/shared/ui/admission";

/** The programs waiting for an answer, each opening its device's page. */
export const Admissions = () => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const selectLink = useUi((s) => s.selectLink);
	const rows = useMemo(
		() =>
			openAdmissions(view).map((admission) => ({
				card: admissionCard(view, admission),
				link: admissionLink(view, admission),
			})),
		[view]
	);
	if (rows.length === 0) return null;
	return (
		<section
			aria-label={t("{{countText}} programs asking to get in", {
				count: rows.length,
				countText: formatNumber(rows.length),
			})}
			className="mx-1 mb-2.5 flex shrink-0 flex-col gap-0.5"
		>
			{rows.map(({ card, link }) => (
				<button
					key={card.id}
					type="button"
					disabled={link === null}
					onClick={() => {
						if (link !== null) selectLink(link);
					}}
					className="flex h-7 items-center gap-2 rounded-md px-1.5 text-left text-[12.5px] text-secondary-foreground hover:bg-accent hover:text-foreground disabled:hover:bg-transparent"
				>
					<ShieldAlert
						aria-hidden
						size={13}
						strokeWidth={2}
						className="shrink-0 text-status-connecting-text"
					/>
					<span className="min-w-0 flex-1 truncate">{card.title}</span>
					<span className="shrink-0 text-[11px] text-muted-foreground">
						{card.deviceName}
					</span>
				</button>
			))}
		</section>
	);
};
