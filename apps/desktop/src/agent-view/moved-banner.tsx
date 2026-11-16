// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The banners of an agent that moved to another device. The old agent
// gets one over its composer, in place of Resume, that opens the new
// agent. The new one gets a line under its header that opens the old one.

import { MonitorDown, MonitorUp } from "lucide-react";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { movedBanner } from "@maxhq/shared/agent-view/move-agent";
import type { Agent } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** Whether an agent moved away, so its banner takes Resume's place. */
export const movedAway = (agent: Agent): boolean => Boolean(agent.moved?.to);

/** Over the composer of an agent that moved to another device. */
export const MovedBanner = ({ agent }: { agent: Agent }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const banner = movedBanner(view, agent);
	if (banner?.way !== "to") return null;
	const { other } = banner;
	return (
		<div className="shrink-0 px-7 pb-2">
			<section
				aria-label={banner.title}
				className="mx-auto flex max-w-[744px] items-center gap-[11px] rounded-[10px] border border-input bg-card py-2.5 pr-3 pl-3.5"
			>
				<span
					aria-hidden
					className="grid size-[26px] shrink-0 place-items-center rounded-md border border-input bg-hover-card text-secondary-foreground"
				>
					<MonitorUp size={13} strokeWidth={2.2} />
				</span>
				<div className="flex min-w-0 flex-1 flex-col gap-[3px]">
					<h3 className="text-[13px] font-semibold text-foreground-strong">
						{banner.title}
					</h3>
					<p className="text-[12px] leading-[1.45] text-muted-foreground">
						{banner.detail}
					</p>
				</div>
				{other ? (
					<button
						type="button"
						onClick={() => openAgent(services, other)}
						className="inline-flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-md bg-primary px-2.5 text-[12px] font-semibold whitespace-nowrap text-primary-foreground shadow-lit hover:bg-primary/90"
					>
						{translate("Open")}
					</button>
				) : null}
			</section>
		</div>
	);
};

/** Under the header of an agent that moved here from another device. */
export const MovedFrom = ({ agent }: { agent: Agent }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const banner = movedBanner(view, agent);
	if (banner?.way !== "from") return null;
	const { other } = banner;
	return (
		<div
			role="note"
			aria-label={banner.title}
			title={banner.detail}
			className="flex min-w-0 items-center gap-1.5 pr-3 pb-2 pl-3.5 text-[11.5px] text-muted-foreground"
		>
			<MonitorDown aria-hidden size={12} strokeWidth={2} className="shrink-0" />
			<span className="truncate">{banner.title}</span>
			{other ? (
				<button
					type="button"
					onClick={() => openAgent(services, other)}
					className="shrink-0 cursor-pointer font-medium text-secondary-foreground underline-offset-2 hover:text-foreground hover:underline"
				>
					{translate("Open the old agent")}
				</button>
			) : null}
		</div>
	);
};
