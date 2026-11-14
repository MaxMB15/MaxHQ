// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pieces the ports popover's sections share: the section with its
// heading, a row's text button and the line that says why an action failed.

import type { ReactNode } from "react";

export const TEXT_BUTTON =
	"inline-flex h-6 cursor-pointer items-center rounded-[5px] border border-input bg-secondary px-2 text-[11.5px] text-foreground outline-none hover:bg-hover-card focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-60";

/** Why a row's last action failed, across the whole row. */
export const Failure = ({ text }: { text: string | null }) =>
	text === null ? null : (
		<p role="alert" className="col-span-full text-[11.5px] text-status-error-text">
			{text}
		</p>
	);

/** A titled list in the popover. */
export const Section = ({ title, children }: { title: string; children: ReactNode }) => (
	<section aria-label={title} className="border-t border-secondary py-1.5">
		<h3 className="px-3.5 pt-1 pb-1 text-[10.5px] font-semibold tracking-[0.04em] text-muted-foreground uppercase">
			{title}
		</h3>
		<ul>{children}</ul>
	</section>
);
