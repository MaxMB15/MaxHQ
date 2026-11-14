// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent strip's chip for a Cursor or ACP agent whose project's skills
// changed since it started. Those CLIs read skills only at start, so the
// agent restarts on its next message. The chip opens the project's Skills
// tab.

import { useI18n } from "@maxhq/shared/i18n";

import { Sparkles } from "lucide-react";

import { useServices, useView } from "@/app/services";
import { openSettings } from "@/settings/store";
import type { AgentId } from "@maxhq/shared/bindings";
import { waitsForSkills } from "@maxhq/shared/skills/library";

export const SkillsRestartChip = ({ agent }: { agent: AgentId }) => {
	const { t } = useI18n();

	const services = useServices();
	const waits = useView((s) => waitsForSkills(s.view, agent));
	const project = useView((s) => {
		const workspace = s.view.tables.agent.get(agent)?.workspace;
		return (workspace && s.view.tables.workspace.get(workspace)?.project) || null;
	});
	if (!waits || !project) return null;
	return (
		<button
			type="button"
			onClick={() => openSettings(services, { kind: "project", project, tab: "skills" })}
			className="inline-flex h-[22px] shrink-0 items-center gap-[5px] rounded-md border border-status-busy/45 bg-status-busy/10 px-[7px] text-[11.5px] whitespace-nowrap text-status-busy-text hover:bg-status-busy/20"
		>
			<Sparkles aria-hidden size={12} strokeWidth={2} />
			{t("Skills changed, restarts on next message")}
		</button>
	);
};
