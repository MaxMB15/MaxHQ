// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The field at the top of the agents that filters them as you type. Enter
// opens the first agent shown, Esc clears it, and a second Esc leaves it.

import { useI18n } from "@maxhq/shared/i18n";

import { useServices } from "@/app/services";
import { ariaKeys, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { FIELD } from "@/settings/controls";

import { FILTER_ID, setFilter, useFilter } from "./filter";

/** The sidebar's filter field. */
export const FilterField = () => {
	const { t } = useI18n();

	const services = useServices();
	const query = useFilter();
	return (
		<input
			id={FILTER_ID}
			type="search"
			aria-label={t("Filter agents")}
			aria-keyshortcuts={ariaKeys(SHORTCUTS.filter)}
			title={t("Filter agents {{value1}}", { value1: keyHint(SHORTCUTS.filter) })}
			placeholder={t("Filter")}
			spellCheck={false}
			autoComplete="off"
			value={query}
			onChange={(e) => setFilter(services, e.target.value)}
			onKeyDown={(e) => {
				if (e.key === "Enter" && query.trim() !== "") {
					// The first row the sidebar shows, past folded groups and
					// closed shelves, opens as a click on it would.
					e.preventDefault();
					e.currentTarget
						.closest("nav")
						?.querySelector<HTMLElement>("[data-agent-row]")
						?.click();
					return;
				}
				if (e.key !== "Escape") return;
				// One Esc does one thing, so it doesn't also end a selection.
				e.preventDefault();
				if (query !== "") setFilter(services, "");
				else e.currentTarget.blur();
			}}
			className={cn(FIELD, "mx-1 mb-1.5 h-7 w-auto shrink-0 text-xs")}
		/>
	);
};
