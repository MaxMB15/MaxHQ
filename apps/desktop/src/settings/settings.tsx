// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The header's gear, and the settings window it opens. Cmd+, opens it too,
// or Ctrl+, off a Mac. Esc leaves a view of its own for the page first,
// and closes the window from the page. Every opening starts the forms and
// the search fresh. What the window holds is in page/.

import { Dialog } from "@base-ui/react/dialog";
import { Settings } from "lucide-react";
import { useEffect } from "react";

import { useServices } from "@/app/services";
import { ariaKeys, holdsCommand, SHORTCUTS } from "@/layout/shortcuts";
import { onMac } from "@/native/platform";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { SettingsWindow } from "./page/window";
import { closeSettings, escapeSettings, openSettings, settingsOf, useSettings } from "./store";

/** The header's gear, with the settings window it opens. */
export const SettingsButton = () => {
	useI18n();
	const services = useServices();
	const open = useSettings((s) => s.open);

	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "," || !holdsCommand(e) || e.shiftKey || e.altKey || e.repeat) return;
			e.preventDefault();
			if (!settingsOf(services).getState().open) openSettings(services);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [services]);

	return (
		<Dialog.Root
			open={open}
			onOpenChange={(next, details) => {
				if (next) return;
				if (details.reason !== "escape-key") return closeSettings(services);
				if (escapeSettings(services)) details.cancel();
			}}
		>
			<button
				type="button"
				aria-label={translate("Settings")}
				aria-keyshortcuts={ariaKeys(SHORTCUTS.settings)}
				title={onMac() ? translate("Settings ⌘,") : translate("Settings Ctrl+,")}
				onClick={() => openSettings(services)}
				className="grid size-[30px] place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
			>
				<Settings aria-hidden size={17} strokeWidth={1.8} />
			</button>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup
					aria-label={translate("Settings")}
					className="fixed top-1/2 left-1/2 z-[121] h-[min(720px,calc(100vh-64px))] w-[min(980px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-xl border border-input bg-card shadow-popup outline-none"
				>
					<SettingsWindow />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
