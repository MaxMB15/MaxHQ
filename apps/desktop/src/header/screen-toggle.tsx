// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sun beside the title. While it's on, the screen doesn't dim and the
// Mac doesn't sleep when idle, on battery too. While it's off, the Keep
// awake setting and macOS decide, as before.

import { t, useI18n } from "@maxhq/shared/i18n";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";

import { Sun, SunDim } from "lucide-react";
import { useEffect, useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { screenOn, setScreenOn } from "@/native/screen";
import { noticesOf } from "@/sidebar/notices";

/** What the sun's tooltip says while it's `on`, or off. */
export const screenTitle = (on: boolean): string =>
	on
		? t("The screen stays on. Click to let it dim and sleep as your settings say")
		: t("Keep the screen on, so it never dims and the Mac never sleeps, on battery too");

/** The sun, or nothing where the screen can't be kept on. */
export const ScreenToggle = () => {
	useI18n();

	const services = useServices();
	const [on, setOn] = useState<boolean | null>(null);
	useEffect(() => {
		let live = true;
		screenOn().then(
			(held) => {
				if (live) setOn(held ?? null);
			},
			(error: unknown) => console.warn("Couldn't read whether the screen stays on", error)
		);
		return () => {
			live = false;
		};
	}, []);
	if (on === null) return null;

	const flip = () => {
		const next = !on;
		setOn(next);
		setScreenOn(next).catch((error: unknown) => {
			setOn(!next);
			const message = error instanceof Error ? error.message : String(error);
			showFailure(
				noticesOf(services),
				next
					? t("Couldn't keep the screen on: {{message}}", { message })
					: t("Couldn't let the screen dim again: {{message}}", { message })
			);
		});
	};
	const Icon = on ? Sun : SunDim;
	return (
		<button
			type="button"
			aria-label={t("Keep the screen on")}
			aria-pressed={on}
			title={screenTitle(on)}
			onClick={flip}
			className={cn(
				"grid size-[30px] shrink-0 place-items-center rounded-[7px] hover:bg-secondary",
				on ? "text-status-busy" : "text-muted-foreground hover:text-secondary-foreground"
			)}
		>
			<Icon aria-hidden size={16} strokeWidth={1.9} />
		</button>
	);
};
