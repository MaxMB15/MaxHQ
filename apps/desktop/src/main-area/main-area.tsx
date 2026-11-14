// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The main area: the agent panes in projects mode, or a project's Pulls
// page over them, the selected device in devices mode, or a line saying
// what's wrong. The panes stay mounted under the Pulls page and the device
// view, so coming back finds them as you left them. In projects mode, the
// PR viewer sits on the right of it all.

import { useI18n } from "@maxhq/shared/i18n";

import {} from "react";

import { useUi, useView } from "@/app/services";
import { ErrorBoundary } from "@/components/error-boundary";
import { useNow } from "@maxhq/shared/lib/clock";
import { AddDeviceButton } from "@/sidebar/add-device";
import { shownDevice } from "@/sidebar/devices";
import { deviceGroups } from "@maxhq/shared/view/select";

import { PullViewer } from "@/pulls/pull-viewer";
import { PullsPage } from "@/pulls/pulls-page";

import { AgentPanes } from "./agent-panes";
import { DeviceDetail } from "./device-detail";
import { ViewFailed, ViewLoading } from "./view-status";

const DevicesMain = () => {
	useI18n();

	const view = useView((s) => s.view);
	const selected = useUi((s) => s.selectedLink);
	const now = useNow(5000);
	const nodes = deviceGroups(view, now).flatMap((g) => g.devices);
	const node = shownDevice(nodes, selected);
	if (!node) return <NoDevices />;
	return <DeviceDetail node={node} view={view} />;
};

/** The devices view with nothing to show, and the way to add the first. */
const NoDevices = () => {
	const { t } = useI18n();
	return (
		<div className="grid h-full place-items-center px-8">
			<div className="flex flex-col items-center gap-3">
				<p className="text-center text-[13px] text-muted-foreground">
					{t("No devices yet.")}
				</p>
				<AddDeviceButton className="inline-flex h-[30px] items-center gap-2 rounded-[7px] border border-input bg-secondary px-3 text-[12.5px] text-foreground hover:bg-accent" />
			</div>
		</div>
	);
};

/** The main area. */
export const MainArea = () => {
	useI18n();

	const mode = useUi((s) => s.mode);
	const pullsOf = useUi((s) => (s.mode === "projects" ? s.pullsOf : null));
	const shownPull = useUi((s) => (s.mode === "projects" ? s.shownPull : null));
	const status = useView((s) => s.status);
	if (status.state === "loading") return <ViewLoading />;
	if (status.state === "failed") {
		return <ViewFailed message={status.message} retryAt={status.retryAt} />;
	}
	return (
		<div className="flex h-full">
			<div className="relative min-w-0 flex-1">
				<AgentPanes hidden={mode !== "projects" || pullsOf !== null} />
				{pullsOf ? (
					<div className="absolute inset-0 bg-canvas">
						<ErrorBoundary part="pull requests">
							<PullsPage key={pullsOf} project={pullsOf} />
						</ErrorBoundary>
					</div>
				) : null}
				{mode === "devices" ? (
					<div className="absolute inset-0 bg-canvas">
						<ErrorBoundary part="device view">
							<DevicesMain />
						</ErrorBoundary>
					</div>
				) : null}
			</div>
			{shownPull ? (
				<ErrorBoundary part="pull request" className="w-[340px] shrink-0">
					<PullViewer shown={shownPull} />
				</ErrorBoundary>
			) : null}
		</div>
	);
};
