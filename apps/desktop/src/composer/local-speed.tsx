// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A local agent's speed line in the composer footer: how fast its model
// server reads the prompt and writes, and how full its context is, such as
// "1,180 / 74 tok/s · 31k of 66k". It stays, dimmed, after the turn. In a
// narrow footer it keeps only the writing speed, and its tooltip and
// accessible name always say all of it in words.

import type { SpeedLine } from "@maxhq/shared/local-models/running";

import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** The speed line, or its short form when `folded`. */
export const LocalSpeed = ({ speed, folded }: { speed: SpeedLine; folded: boolean }) => (
	<Tooltip content={speed.full}>
		<span
			data-local-speed={speed.live ? "live" : "last"}
			className={cn(
				"shrink-0 px-1 text-[11.5px] whitespace-nowrap tabular-nums",
				speed.live ? "text-muted-foreground" : "text-subtle-foreground"
			)}
		>
			<span aria-hidden>{folded ? speed.short : speed.text}</span>
			<span className="sr-only">{speed.full}</span>
		</span>
	</Tooltip>
);
