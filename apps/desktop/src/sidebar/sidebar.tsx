// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The left column: a switch between projects and devices, the programs
// devices ask you to let in, the list for the one picked, and the buttons
// at the bottom. In projects, a field above the list filters the agents,
// the bar for the agents you selected and the inbox notice sit above the
// buttons, and Escape ends the selection.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useEffect } from "react";

import { useServices, useUi } from "@/app/services";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ariaKeys, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { openNewAgent } from "@/new-agent/store";
import { SkillChanges } from "@/skills/skill-changes";
import type { SidebarMode } from "@maxhq/shared/ui/store";

import { AddDeviceButton } from "./add-device";
import { Admissions } from "./admissions";
import { CustomSnoozeHost } from "./custom-snooze";
import { DeviceList } from "./devices";
import { FilterField } from "./filter-field";
import { SelectionBar } from "./inbox-menu";
import { InboxNotice } from "./notice";
import { NeedsYou, ProjectList } from "./projects";
import { SIDEBAR_ID } from "./toggle";
import { UpdateCapsule } from "./update-capsule";

/** How often the rows' times and dots move on. */
const ROW_CLOCK_MS = 5000;

const MODES: { mode: SidebarMode; label: string }[] = [
	{
		mode: "projects",
		get label() {
			return t("Projects");
		},
	},
	{
		mode: "devices",
		get label() {
			return t("Devices");
		},
	},
];

const ModeSwitch = () => {
	const { t } = useI18n();

	const mode = useUi((s) => s.mode);
	const setMode = useUi((s) => s.setMode);
	return (
		<div
			role="group"
			aria-label={t("Show")}
			className="mx-1 mb-2 flex shrink-0 gap-0.5 rounded-[7px] border border-sidebar-border bg-background p-0.5"
		>
			{MODES.map((m) => (
				<button
					key={m.mode}
					type="button"
					aria-pressed={mode === m.mode}
					onClick={() => setMode(m.mode)}
					className={cn(
						"h-6 flex-1 rounded-[5px] text-xs",
						mode === m.mode
							? "bg-border font-[550] text-foreground"
							: "text-muted-foreground hover:text-secondary-foreground"
					)}
				>
					{m.label}
				</button>
			))}
		</div>
	);
};

const Icon = ({ path, size = 13 }: { path: string; size?: number }) => {
	useI18n();
	return (
		<svg
			aria-hidden
			width={size}
			height={size}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2}
			strokeLinecap="round"
			strokeLinejoin="round"
			className="shrink-0"
		>
			<path d={path} />
		</svg>
	);
};

const PLUS = "M12 5v14M5 12h14";
const REFRESH = "M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5";

const bordered =
	"mx-1 mt-1.5 flex h-[30px] shrink-0 items-center gap-2 rounded-[7px] border border-track px-2 text-xs text-secondary-foreground disabled:opacity-60 enabled:hover:bg-accent";

const ProjectsFooter = () => {
	const { t } = useI18n();

	const services = useServices();
	const key = keyHint(SHORTCUTS.newAgent);
	return (
		<button
			type="button"
			title={t("New agent {{key}}", { key: key })}
			aria-keyshortcuts={ariaKeys(SHORTCUTS.newAgent)}
			onClick={() => openNewAgent(services)}
			className={bordered}
		>
			<Icon path={PLUS} />
			{t("New agent")}
			<span className="ml-auto text-[11px] text-muted-foreground">{key}</span>
		</button>
	);
};

const DevicesFooter = () => {
	const { t } = useI18n();

	const { source } = useServices();
	return (
		<>
			<UpdateCapsule />
			<AddDeviceButton className={bordered} />
			<button
				type="button"
				onClick={() => void source.refreshLinks(null)}
				className="mx-1 flex h-7 shrink-0 items-center gap-2 rounded-md px-2 text-xs text-muted-foreground hover:text-secondary-foreground"
			>
				<Icon path={REFRESH} size={12} />
				{t("Refresh all")}
			</button>
		</>
	);
};

/** The sidebar. */
export const Sidebar = () => {
	const { t } = useI18n();

	const services = useServices();
	const mode = useUi((s) => s.mode);
	const now = useNow(ROW_CLOCK_MS);
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape" || e.defaultPrevented) return;
			const ui = services.ui.getState();
			if (ui.picked.size > 0) ui.clearPicked();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [services]);
	return (
		<nav
			id={SIDEBAR_ID}
			aria-label={mode === "projects" ? t("Agents") : t("Devices")}
			className="flex w-full min-w-0 flex-col border-r border-sidebar-border bg-sidebar px-1.5 py-2"
		>
			<ModeSwitch />
			<Admissions />
			{mode === "projects" ? (
				<>
					<NeedsYou />
					<SkillChanges />
					<FilterField />
				</>
			) : null}
			<ScrollArea className="flex-1" viewportClassName="flex flex-col gap-0.5">
				{mode === "projects" ? <ProjectList now={now} /> : <DeviceList now={now} />}
			</ScrollArea>
			{mode === "projects" ? (
				<>
					<SelectionBar now={now} />
					<InboxNotice />
					<ProjectsFooter />
				</>
			) : (
				<DevicesFooter />
			)}
			<CustomSnoozeHost />
		</nav>
	);
};
