// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A small heading inside a group of the Settings page, over the cards
// that belong together, such as Between agents in Permissions.

import { useI18n } from "@maxhq/shared/i18n";

export const Subheading = ({ title, hint }: { title: string; hint?: string }) => {
	useI18n();
	return (
		<div className="flex flex-wrap items-baseline gap-x-2 px-1 pt-1">
			<h3 className="text-[12.5px] font-[600] text-foreground">{title}</h3>
			{hint ? <p className="text-[11.5px] text-muted-foreground">{hint}</p> : null}
		</div>
	);
};
