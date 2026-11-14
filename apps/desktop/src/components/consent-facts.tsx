// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The details a card that asks for your consent gives to judge it by, each
// a muted label beside its value.

/** One labelled detail, such as `Container: web`. */
export type ConsentFact = { readonly label: string; readonly value: string };

export const ConsentFacts = ({ facts }: { facts: readonly ConsentFact[] }) => (
	<dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[11.5px]">
		{facts.map((fact, i) => (
			// A label can come more than once, such as each agent a call reaches,
			// and the list doesn't change, so its place is its identity.
			<div key={i} className="contents">
				<dt className="text-muted-foreground">{fact.label}</dt>
				<dd className="min-w-0 font-mono break-all text-secondary-foreground select-text">
					{fact.value}
				</dd>
			</div>
		))}
	</dl>
);
