// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES: the
// banner for a folder whose clone is still running, with Cancel, or that failed
// or was cancelled, with Retry and Remove, and for the files a copy couldn't
// place there until you dismiss them. Agents can't start in the folder until
// the clone is done.

import { Download, FileX } from "lucide-react";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import type { WorkspaceId } from "@maxhq/shared/bindings";
import type { ActionResult, CloneState, PlaceFailure } from "@maxhq/shared/data/source";
import { cn } from "@/lib/utils";
import { plural, sentence } from "@maxhq/shared/status/format";
import { dismissMissed, useCopy } from "@/workspaces/copy-store";
import { cloneOf, cloneWording } from "@maxhq/shared/workspaces/state";

import { failureOf, type SendState } from "@maxhq/shared/agent-view/answer";
import { Spinner } from "./icons";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const BUTTON =
	"inline-flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2.5 text-[12px] whitespace-nowrap disabled:cursor-default disabled:opacity-60";
const QUIET = cn(BUTTON, "border border-input bg-secondary text-foreground hover:bg-accent");
const LOUD = cn(
	BUTTON,
	"bg-primary font-semibold text-primary-foreground shadow-lit hover:bg-primary/90"
);

/** A bar for how far something has got, out of 100. */
export const Bar = ({ label, percent }: { label: string; percent: number | null }) => {
	useI18n();
	return (
		<div
			role="progressbar"
			aria-label={label}
			aria-valuemin={0}
			aria-valuemax={100}
			aria-valuenow={percent ?? undefined}
			className="h-1 overflow-hidden rounded-full bg-track"
		>
			<div
				className={cn(
					"h-full rounded-full bg-status-connecting transition-[width] duration-300",
					percent === null && "animate-pulse"
				)}
				style={{ width: `${percent ?? 100}%` }}
			/>
		</div>
	);
};

/** The section the banner's two looks share. */
const Frame = ({
	label,
	icon,
	title,
	children,
	actions,
	failure,
}: {
	label: string;
	icon: React.ReactNode;
	title: string;
	children: React.ReactNode;
	actions: React.ReactNode;
	failure: string | null;
}) => {
	useI18n();
	return (
		<section
			aria-label={label}
			className="mx-auto flex w-full max-w-[744px] items-start gap-[11px] rounded-[10px] border border-input bg-card py-2.5 pr-3 pl-3.5"
		>
			<span
				aria-hidden
				className="mt-px grid size-[26px] shrink-0 place-items-center rounded-md border border-input bg-hover-card text-secondary-foreground"
			>
				{icon}
			</span>
			<div className="flex min-w-0 flex-1 flex-col gap-[3px]">
				<h3 className="text-[13px] font-semibold text-foreground-strong">{title}</h3>
				{children}
				{failure ? (
					<p role="alert" className="text-[11.5px] text-status-error-text">
						{failure}
					</p>
				) : null}
			</div>
			<div className="flex shrink-0 items-center gap-1.5 self-center">{actions}</div>
		</section>
	);
};

/** The files a copy couldn't place, until you dismiss them. */
const Missed = ({
	workspace,
	failed,
}: {
	workspace: WorkspaceId;
	failed: readonly PlaceFailure[];
}) => {
	useI18n();
	const services = useServices();
	return (
		<Frame
			label={translate("Files not copied")}
			icon={<FileX size={13} strokeWidth={2.2} />}
			title={translate("{{plural}} {{value2}} copied", {
				plural: plural(failed.length, "file"),
				value2: failed.length === 1 ? "wasn't" : "weren't",
			})}
			failure={null}
			actions={
				<button
					type="button"
					className={QUIET}
					onClick={() => dismissMissed(services, workspace)}
				>
					{translate("Dismiss")}
				</button>
			}
		>
			<ul className="flex max-h-28 flex-col gap-px overflow-y-auto">
				{failed.map((f) => (
					<li
						key={f.path}
						className="flex items-baseline gap-2 text-[12px] leading-[1.45]"
					>
						<span className="min-w-0 truncate font-mono text-[11.5px] text-terminal">
							{f.path}
						</span>
						<span className="min-w-0 flex-1 truncate text-muted-foreground">
							{sentence(f.reason)}
						</span>
					</li>
				))}
			</ul>
		</Frame>
	);
};

/** The clone's state, with what you can do about it. */
const Clone = ({ workspace, clone }: { workspace: WorkspaceId; clone: CloneState }) => {
	useI18n();
	const { source } = useServices();
	const [state, setState] = useState<SendState>({ state: "idle" });
	const busy = state.state === "sending";
	const { title, body } = cloneWording(clone);

	const run = (action: (id: WorkspaceId) => Promise<ActionResult>, fallback: string) => {
		setState({ state: "sending" });
		action(workspace).then(
			(result) => {
				const failure = failureOf(result);
				setState(failure ? { state: "failed", message: failure } : { state: "idle" });
			},
			(error: unknown) =>
				setState({
					state: "failed",
					message: error instanceof Error ? error.message : fallback,
				})
		);
	};
	const spinner = busy ? (
		<Spinner size={11} className="border-primary-foreground/30 border-t-primary-foreground" />
	) : null;

	return (
		<Frame
			label={translate("Clone")}
			icon={<Download size={13} strokeWidth={2.2} />}
			title={title}
			failure={state.state === "failed" ? state.message : null}
			actions={
				clone.state === "cloning" ? (
					<button
						type="button"
						className={QUIET}
						disabled={busy}
						onClick={() =>
							run(source.cancelClone, translate("Couldn't cancel the clone."))
						}
					>
						{translate("Cancel")}
					</button>
				) : (
					<>
						<button
							type="button"
							className={QUIET}
							disabled={busy}
							onClick={() =>
								run(source.removeClone, translate("Couldn't remove the folder."))
							}
						>
							{translate("Remove")}
						</button>
						<button
							type="button"
							className={LOUD}
							disabled={busy}
							onClick={() =>
								run(source.retryClone, translate("Couldn't retry the clone."))
							}
						>
							{spinner}
							{translate("Retry")}
						</button>
					</>
				)
			}
		>
			<p className="text-[12px] leading-[1.45] text-muted-foreground">{body}</p>
			{clone.state === "cloning" ? (
				<div className="mt-1">
					<Bar label={translate("Clone")} percent={clone.percent} />
				</div>
			) : null}
		</Frame>
	);
};

/**
 * A workspace's clone banner, or nothing once the clone is done and every
 * file it was copied with landed.
 */
export const CloneBanner = ({
	workspace,
	className,
}: {
	workspace: WorkspaceId;
	className?: string;
}) => {
	useI18n();
	const clone = useView((s) => {
		const found = s.view.tables.workspace.get(workspace);
		return found ? cloneOf(found) : null;
	});
	const missed = useCopy((s) => s.missed.get(workspace));
	if (!clone && !missed?.length) return null;
	return (
		<div className={cn("flex flex-col gap-2", className)}>
			{clone ? <Clone workspace={workspace} clone={clone} /> : null}
			{missed?.length ? <Missed workspace={workspace} failed={missed} /> : null}
		</div>
	);
};
