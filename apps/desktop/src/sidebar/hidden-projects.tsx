// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The projects you hid from the sidebar. A hidden project's group shows
// only while one of its agents calls it back, and the Hidden projects entry
// under the groups lists each one with a way to show it again.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { EyeOff } from "lucide-react";
import { useState } from "react";

import { type Services, useServices } from "@/app/services";
import { failureOf } from "@/settings/result";
import type { Project } from "@maxhq/shared/bindings";
import { sentence } from "@maxhq/shared/status/format";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";

import { Chevron } from "./agent-row";
import { noticesOf } from "./notices";

/** Hides the project on every device, or shows it again, saying what went wrong. */
export const setHidden = async (services: Services, project: Project, hidden: boolean) => {
	const failure = failureOf(await services.source.setProjectHidden(project.id, hidden));
	if (failure === null) return;
	const tried = t("Couldn't {{value1}} {{name}}", {
		value1: hidden ? "hide" : "show",
		name: project.name,
	});
	showFailure(noticesOf(services), `${tried}. ${sentence(failure)}.`);
};

/** The Hidden projects entry, closed at first. Absent while none are hidden. */
export const HiddenProjects = ({ projects }: { projects: readonly Project[] }) => {
	const { t } = useI18n();

	const services = useServices();
	const [open, setOpen] = useState(false);
	if (projects.length === 0) return null;
	return (
		<section aria-label={t("Hidden projects")} className="mt-2 flex flex-col gap-0.5">
			<button
				type="button"
				aria-expanded={open}
				onClick={() => setOpen(!open)}
				className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-md px-2 text-left text-xs font-semibold text-muted-foreground hover:text-secondary-foreground"
			>
				<Chevron closed={!open} />
				<EyeOff aria-hidden size={11} strokeWidth={2.2} className="shrink-0" />
				<span className="min-w-0 flex-1 truncate">{t("Hidden projects")}</span>
				<span className="shrink-0 text-[11px] font-normal tabular-nums">
					{formatNumber(projects.length)}
				</span>
			</button>
			{open
				? projects.map((project) => (
						<div
							key={project.id}
							className="flex h-6 shrink-0 items-center gap-2 rounded-md pr-1 pl-[22px] text-[12px] text-secondary-foreground"
						>
							<span className="min-w-0 flex-1 truncate">{project.name}</span>
							<button
								type="button"
								aria-label={t("Show project {{name}}", { name: project.name })}
								onClick={() => void setHidden(services, project, false)}
								className="shrink-0 rounded-[5px] px-1.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
							>
								{t("Show project")}
							</button>
						</div>
					))
				: null}
		</section>
	);
};
