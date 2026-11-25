// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The window: the header, the sidebar with the sub-agent panel beside it
// when one is open, the main area, the status bar, the new-agent form and
// the list of keyboard shortcuts.
// An error in the sidebar, the panel or the main area stays in its spot,
// and one anywhere else takes the window until you try again.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { ServicesProvider, useUi, useView } from "@/app/services";
import { StatusBar } from "@/app/status-bar";
import { ConfirmHost } from "@/components/confirm";
import { ErrorBoundary } from "@/components/error-boundary";
import { createSource, type DataSource } from "@/data";
import { Header } from "@/header/header";
import { useNow } from "@maxhq/shared/lib/clock";
import { MainArea } from "@/main-area/main-area";
import { NewAgent } from "@/new-agent/new-agent";
import { ShortcutSheet } from "@/shortcut-list/sheet";
import { SidebarFrame } from "@/sidebar/frame";
import { Sidebar } from "@/sidebar/sidebar";
import { cn } from "@/lib/utils";
import { AppKeyHomePrompt } from "@/settings/app-key-home";
import { CliSignInHost } from "@/settings/cli-sign-in";
import { SlideOutFor } from "@/workflow/containers";
import type { UiStore } from "@maxhq/shared/ui/store";
import { runName, runsOf } from "@maxhq/shared/workflow/select";
import { UpdatePrompt } from "@/updates/update-prompt";
import { CopyProjectHost } from "@/workspaces/copy-project";

const SubAgents = () => {
	useI18n();

	const mode = useUi((s) => s.mode);
	const slide = useUi((s) => s.slideOut);
	const lead = useView((s) => (slide?.lead ? s.view.tables.agent.get(slide.lead) : undefined));
	// Run mode: a run no agent leads yet, with no lead above it.
	const runMode = slide !== null && slide.lead === null && slide.run !== null ? slide.run : null;
	// The fallback keeps the panel's width, so an error never moves the main area.
	const wide = useView((s) => (lead ? runsOf(s.view, lead.id).length > 0 : runMode !== null));
	const now = useNow(5000);
	if (mode !== "projects" || (!lead && runMode === null)) return null;
	return (
		<ErrorBoundary
			key={lead?.id ?? (runMode ? runName(runMode) : "")}
			part="sub-agent panel"
			className={cn(
				"h-auto shrink-0 border-r border-sidebar-border bg-subpanel px-4",
				// The panel's own width rule, from `slideOutWidth`.
				wide ? "w-[320px]" : "w-[216px]"
			)}
		>
			<SlideOutFor now={now} />
		</ErrorBoundary>
	);
};

/** The window's layout, inside a `ServicesProvider`. */
export const Shell = () => {
	const { t } = useI18n();

	const mode = useUi((s) => s.mode);
	return (
		<div className="flex h-full flex-col">
			<Header />
			<div className="flex min-h-0 flex-1">
				<SidebarFrame>
					<Sidebar />
				</SidebarFrame>
				<SubAgents />
				<main
					aria-label={mode === "projects" ? t("Agent") : t("Device")}
					className="min-w-0 flex-1 bg-canvas"
				>
					<ErrorBoundary part="main area">
						<MainArea />
					</ErrorBoundary>
				</main>
			</div>
			<StatusBar />
			<NewAgent />
			<ShortcutSheet />
			<CopyProjectHost />
			<CliSignInHost />
			<ConfirmHost />
			<UpdatePrompt />
			<AppKeyHomePrompt />
		</div>
	);
};

/** The app, on the source this build picks unless a test passes one. */
const App = ({ source, ui }: { source?: DataSource; ui?: UiStore }) => {
	useI18n();

	const [picked] = useState(() => source ?? createSource());
	return (
		<ErrorBoundary part="window" className="bg-canvas">
			<ServicesProvider source={picked} ui={ui}>
				<Shell />
			</ServicesProvider>
		</ErrorBoundary>
	);
};

export default App;
