// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A value in Settings, such as an address, a path or a version: a small
// mono chip you can select, with a copy button that shows while you hover
// the value's row or tab to it. A long value cuts off and shows all of
// itself on hover.

import { CopyIcon } from "@/components/copy-field";
import { cn } from "@/lib/utils";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The chip's look, for a value with no copy button. */
export const VALUE =
	"selectable min-w-0 truncate rounded-[6px] border border-input bg-subpanel px-[7px] py-0.5 font-mono text-[11.5px] text-foreground";

/** `text` in a chip, with a copy button named for `what`, such as "Address". */
export const Value = ({
	text,
	what,
	className,
}: {
	text: string;
	what: string;
	className?: string;
}) => {
	useI18n();
	return (
		<span className={cn("group/value inline-flex min-w-0 items-center gap-1", className)}>
			<span title={text} className={VALUE}>
				{text}
			</span>
			<CopyIcon
				text={text}
				label={translate("Copy {{what}}", { what })}
				className="opacity-0 group-hover/value:opacity-100 focus-visible:opacity-100"
			/>
		</span>
	);
};
