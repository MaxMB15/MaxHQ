// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The details a card that asks for your consent gives to judge it by, each
// a muted label beside its value. A long value, such as a task's whole
// prompt, keeps its lines and scrolls in its box, and says when there's more
// below, so none of it goes unread.

import { useMoreBelow } from "@/lib/use-more-below";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** One labelled detail, such as `Container: web`. */
export type ConsentFact = { readonly label: string; readonly value: string };

const Value = ({ value }: { value: string }) => {
	useI18n();
	const { ref, more, measure } = useMoreBelow<HTMLDivElement>(value);
	return (
		<dd className="flex min-w-0 flex-col gap-0.5">
			<div
				ref={ref}
				onScroll={measure}
				className="selectable max-h-[220px] overflow-auto font-mono break-all whitespace-pre-wrap text-secondary-foreground"
			>
				{value}
			</div>
			{more ? (
				<span className="text-muted-foreground">
					{translate("More below. Scroll to read it all.")}
				</span>
			) : null}
		</dd>
	);
};

export const ConsentFacts = ({ facts }: { facts: readonly ConsentFact[] }) => (
	<dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[11.5px]">
		{facts.map((fact, i) => (
			// A label can come more than once, such as each agent a call reaches,
			// and the list doesn't change, so its place is its identity.
			<div key={i} className="contents">
				<dt className="text-muted-foreground">{fact.label}</dt>
				<Value value={fact.value} />
			</div>
		))}
	</dl>
);
