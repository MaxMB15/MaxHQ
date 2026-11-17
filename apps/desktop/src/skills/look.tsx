// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pieces the skills views share: a skill's badges, the meter for a
// CLI's listing, and the hook that runs a skills request and says how it
// went under the buttons.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useState } from "react";

import { failureOf, type Note } from "@/settings/result";
import { Badge } from "@/tasks/activity";
import { cn } from "@/lib/utils";
import type { ActionResult } from "@maxhq/shared/data/source";
import type { Budget } from "@maxhq/shared/skills/effective";
import { badgeLabel, SKILL_BADGE_TONE, type SkillBadge } from "@maxhq/shared/skills/library";

/** A skill's badges in a row, or nothing for none. */
export const SkillBadges = ({ badges }: { badges: readonly SkillBadge[] }) => {
	useI18n();
	return badges.length > 0 ? (
		<span className="flex shrink-0 flex-wrap gap-1">
			{badges.map((badge) => (
				<Badge key={badge} tone={SKILL_BADGE_TONE[badge]}>
					{badgeLabel(badge)}
				</Badge>
			))}
		</span>
	) : null;
};

/** How much of a CLI's listing its skills take, or how many it lists when it has no size. */
export const BudgetMeter = ({ budget }: { budget: Budget }) => {
	const { t } = useI18n();

	const { limit, used, over, listed } = budget;
	if (limit === null) {
		return (
			<span className="text-[11.5px] text-muted-foreground">
				{t("{{listed}} listed", { listed: formatNumber(listed) })}
			</span>
		);
	}
	const percent = Math.min(100, (used / limit) * 100);
	const text = t("{{value1}} of {{value2}} characters", {
		value1: formatNumber(used),
		value2: formatNumber(limit),
	});
	return (
		<span
			role="img"
			aria-label={t("Skill listing, {{text}}{{value2}}", {
				text: text,
				value2: over ? t(", over") : "",
			})}
			title={over ? t("Past the listing, so the CLI drops descriptions") : undefined}
			className="inline-flex items-center gap-1.5 text-[11.5px] text-muted-foreground tabular-nums"
		>
			<span aria-hidden className="h-1 w-12 overflow-hidden rounded-[2px] bg-track">
				<span
					className={cn("block h-1", over ? "bg-status-error" : "bg-primary")}
					style={{ width: `${percent}%` }}
				/>
			</span>
			{text}
		</span>
	);
};

/**
 * Runs skills requests one at a time and keeps a note of how the last
 * went. `run` says whether the request went through.
 */
export const useSkillAction = () => {
	const [note, setNote] = useState<Note | null>(null);
	const [busy, setBusy] = useState(false);
	const run = async (
		doing: string,
		done: string,
		action: () => Promise<ActionResult>
	): Promise<boolean> => {
		if (busy) return false;
		setBusy(true);
		setNote({ text: doing, error: false });
		try {
			const failure = failureOf(await action());
			setNote(failure ? { text: failure, error: true } : { text: done, error: false });
			return failure === null;
		} catch (error) {
			console.warn("A skills request failed", error);
			setNote({ text: t("Couldn't reach the device. Try again."), error: true });
			return false;
		} finally {
			setBusy(false);
		}
	};
	return { note, busy, run, setNote };
};
