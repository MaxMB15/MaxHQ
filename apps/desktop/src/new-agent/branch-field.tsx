// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The branch an agent in a container works on. It's free text, and empty
// lets `maxd` name a new branch. As you type it suggests the branches the
// project's agents work on, its open PRs' heads and the repo's other
// branches, and it says when another agent in the container has the
// branch, since they'll share its folder. A device that can't list the
// repo's branches leaves the field with what the app already knows.

import { Autocomplete } from "@base-ui/react/autocomplete";
import { useMemo, useState } from "react";

import { useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { useBranches, usePulls } from "@/pulls/queries";
import type { ProjectId, WorkspaceId } from "@maxhq/shared/bindings";
import {
	agentsOnBranch,
	branchProblem,
	branchSuggestions,
	type BranchSuggestion,
	branchTag,
	filterBranches,
	folderClash,
	SHARED_BRANCH_NOTE,
} from "@maxhq/shared/new-agent/branch";

import { AGENT_FIELD } from "./agent-pickers";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The branch field, with what it suggests and its note. */
export const BranchField = ({
	project,
	workspace,
	value,
	onChange,
}: {
	project: ProjectId;
	workspace: WorkspaceId;
	value: string;
	onChange: (value: string) => void;
}) => {
	useI18n();
	const view = useView((s) => s.view);
	const pulls = usePulls(project, "open");
	const repo = useBranches(project);
	const suggestions = useMemo(
		() => branchSuggestions(view, project, pulls.data ?? [], repo.data),
		[view, project, pulls.data, repo.data]
	);
	const shown = useMemo(() => filterBranches(suggestions, value), [suggestions, value]);
	// An open list hides the rest of the form from screen readers, so it
	// opens only when something matches.
	const [open, setOpen] = useState(false);
	const name = value.trim();
	const bad =
		name !== "" &&
		(branchProblem(name) !== null || folderClash(view, workspace, name) !== null);
	const shared = name !== "" && !bad && agentsOnBranch(view, workspace, name).length > 0;

	return (
		<div className="flex flex-col gap-1.5">
			<Autocomplete.Root
				items={suggestions}
				filteredItems={shown}
				open={open && shown.length > 0}
				onOpenChange={setOpen}
				value={value}
				onValueChange={(next) => onChange(next)}
				itemToStringValue={(s: BranchSuggestion) => s.name}
			>
				<label className="flex flex-col gap-1.5">
					<span className="text-[11.5px] font-[550] text-secondary-foreground">
						{translate("Branch")}
					</span>
					<Autocomplete.Input
						placeholder={translate("A new maxhq/ branch")}
						spellCheck={false}
						autoComplete="off"
						aria-invalid={bad}
						className={cn(
							AGENT_FIELD,
							"h-8 font-mono text-xs aria-invalid:border-status-error/60"
						)}
					/>
				</label>
				<Autocomplete.Portal>
					<Autocomplete.Positioner
						sideOffset={6}
						align="start"
						className="z-[140] outline-none"
					>
						<Autocomplete.Popup className="max-h-[min(280px,var(--available-height))] w-(--anchor-width) overflow-y-auto rounded-[10px] border border-input bg-popover p-[5px] text-foreground shadow-menu outline-none">
							<Autocomplete.List className="flex flex-col">
								{(s: BranchSuggestion) => {
									const tag = branchTag(s);
									return (
										<Autocomplete.Item
											key={s.name}
											value={s}
											className="flex cursor-default items-center gap-2.5 rounded-md px-2 py-[6px] outline-none select-none data-highlighted:bg-accent"
										>
											<span className="min-w-0 flex-1 truncate font-mono text-xs">
												{s.name}
											</span>
											{tag ? (
												<span className="h-[18px] shrink-0 rounded-[4px] bg-border px-1.5 text-[10.5px] leading-[18px] text-secondary-foreground">
													{tag}
												</span>
											) : null}
										</Autocomplete.Item>
									);
								}}
							</Autocomplete.List>
						</Autocomplete.Popup>
					</Autocomplete.Positioner>
				</Autocomplete.Portal>
			</Autocomplete.Root>
			{shared ? (
				<p className="text-[11.5px] text-muted-foreground">{SHARED_BRANCH_NOTE}</p>
			) : null}
		</div>
	);
};
