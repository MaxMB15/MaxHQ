// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's Setup: what its last host check found, item by item, with
// the fix for each. MaxHQ applies what maxd can do itself. The part that
// needs root is one command, shown with Copy and a button that opens it in
// Terminal, where you type the sudo password. A NixOS host gets the change
// to make instead.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";
import { openRootFix, opensTerminal } from "@/native/terminal";
import { PRIMARY, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	type FixView,
	fixableNow,
	hostGroups,
	type HostGroup,
	hostReportOf,
	offersHostSpec,
	sourceText,
} from "@maxhq/shared/hosts/report";
import { formatWhen, plural } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";

/** One root command and the items it fixes. */
type RootPart = { readonly line: string; readonly item: string; readonly labels: string[] };

/** The distinct root commands in `groups`, each with the items it fixes. */
export const rootParts = (groups: readonly HostGroup[]): RootPart[] => {
	const parts = new Map<string, RootPart>();
	for (const item of groups.flatMap((g) => g.items)) {
		if (item.ok || item.fix.kind !== "terminal") continue;
		const part = parts.get(item.fix.line);
		if (part) part.labels.push(item.label);
		else parts.set(item.fix.line, { line: item.fix.line, item: item.id, labels: [item.label] });
	}
	return [...parts.values()];
};

const FIX_TEXT: Readonly<Record<FixView["kind"], string>> = {
	none: "",
	get now() {
		return t("Apply fixes it");
	},
	get terminal() {
		return t("The root command below fixes it");
	},
	nix: "",
};

const Code = ({ text }: { text: string }) => {
	useI18n();
	return (
		<code className="block min-w-0 flex-1 rounded-[6px] border border-border bg-subpanel px-2.5 py-1.5 font-mono text-[11.5px] break-all whitespace-pre-wrap text-foreground select-text">
			{text}
		</code>
	);
};

const Group = ({ group }: { group: HostGroup }) => {
	const { t } = useI18n();
	return (
		<div role="group" aria-label={group.label} className="border-t border-sidebar-border">
			<div className="flex items-baseline justify-between px-4 pt-2.5 pb-1">
				<h3 className="text-xs font-[650] text-secondary-foreground">{group.label}</h3>
				<span className="text-[11.5px] text-muted-foreground">
					{group.problems > 0
						? t("{{value1}} to fix", { value1: plural(group.problems, "item") })
						: t("All ok")}
				</span>
			</div>
			{group.items.map((item) => (
				<div key={item.id} className="flex flex-col gap-1.5 px-4 py-2">
					<div className="flex items-center gap-3">
						<span
							role="img"
							aria-label={item.ok ? t("Ok") : t("Needs a fix")}
							className={cn(
								"size-2 shrink-0 rounded-full",
								item.ok ? "bg-status-ready" : "bg-status-error"
							)}
						/>
						<span className="w-[180px] shrink-0 truncate text-[12.5px] text-foreground">
							{item.label}
						</span>
						<span
							className={cn(
								"min-w-0 flex-1 truncate text-xs",
								item.ok ? "text-muted-foreground" : "text-status-error-text"
							)}
						>
							{item.status}
						</span>
						{!item.ok && FIX_TEXT[item.fix.kind] ? (
							<span className="shrink-0 text-[11.5px] text-muted-foreground">
								{FIX_TEXT[item.fix.kind]}
							</span>
						) : null}
					</div>
					{!item.ok && item.fix.kind === "nix" ? (
						<div className="flex items-start gap-2 pl-5">
							<Code text={item.fix.change} />
							<CopyButton
								text={item.fix.change}
								label={t("Copy change")}
								icon={false}
								className={SECONDARY}
							/>
						</div>
					) : null}
				</div>
			))}
		</div>
	);
};

const RootCommand = ({
	device,
	name,
	part,
	live,
}: {
	device: DeviceId;
	name: string;
	part: RootPart;
	live: boolean;
}) => {
	const { t } = useI18n();

	const [note, setNote] = useState<Note | null>(null);
	const open = async () => {
		setNote({ text: t("Opening Terminal"), error: false });
		try {
			const failure = await openRootFix(device, part.item);
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't open Terminal", error);
			setNote({ text: t("Couldn't open Terminal. Copy the command instead."), error: true });
		}
	};
	return (
		<div
			role="group"
			aria-label={t("Root command")}
			className="flex flex-col gap-2 border-t border-sidebar-border bg-subpanel/50 px-4 py-3"
		>
			<div className="flex flex-col gap-0.5">
				<span className="text-[12.5px] text-foreground">
					{t("Run as root on {{name}}", { name: name })}
				</span>
				<span className="text-xs text-muted-foreground">
					{t(
						"Fixes {{value2}}. You type the sudo password in Terminal, and MaxHQ never sees it. Check again once it ends.",
						{ value2: part.labels.join(", ") }
					)}
				</span>
			</div>
			<div className="flex items-start gap-2">
				<Code text={part.line} />
				<CopyButton
					text={part.line}
					label={t("Copy command")}
					icon={false}
					className={SECONDARY}
				/>
				{opensTerminal() ? (
					<button
						type="button"
						className={SECONDARY}
						disabled={!live}
						onClick={() => void open()}
					>
						{t("Open in Terminal")}
					</button>
				) : null}
			</div>
			<StatusNote note={note} />
		</div>
	);
};

/** The device's Setup section, from its last host check. */
export const DeviceSetup = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(60_000);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	if (!offersHostSpec(view, device)) return null;
	const report = hostReportOf(view, device);
	const groups = report ? hostGroups(view, report) : [];
	const fixable = report ? fixableNow(report) : [];
	const roots = rootParts(groups);

	const run = async (what: string, call: () => ReturnType<typeof source.checkHost>) => {
		setBusy(true);
		setNote({ text: what, error: false });
		try {
			const failure = failureOf(await call());
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't reach the device for its host", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	return (
		<Section
			title={t("Setup")}
			label={t("Setup of {{name}}", { name: name })}
			hint={
				report
					? t("{{value1}}. Checked {{value2}}", {
							value1: sourceText(report.source),
							value2: formatWhen(report.checked_at, now),
						})
					: t("Not checked yet")
			}
		>
			<Row
				title={
					report
						? fixable.length > 0 || roots.length > 0
							? t("{{name}} needs some setup", { name: name })
							: t("{{name}} is set up", { name: name })
						: t("MaxHQ hasn't checked {{name}} yet", { name: name })
				}
				sub={
					fixable.length > 0
						? t("Apply fixes {{value1}} without root.", {
								value1: plural(fixable.length, "item"),
							})
						: live
							? undefined
							: t("Connect to {{name}} to check it.", { name: name })
				}
			>
				<StatusNote note={note} />
				<button
					type="button"
					className={SECONDARY}
					disabled={!live || busy}
					onClick={() => void run(t("Checking"), () => source.checkHost(device))}
				>
					{report ? t("Check again") : t("Check")}
				</button>
				{fixable.length > 0 ? (
					<button
						type="button"
						className={PRIMARY}
						disabled={!live || busy}
						onClick={() =>
							void run(t("Applying"), () => source.applyHost(device, fixable))
						}
					>
						{t("Apply")}
					</button>
				) : null}
			</Row>
			{groups.map((group) => (
				<Group key={group.id} group={group} />
			))}
			{roots.map((part) => (
				<RootCommand key={part.line} device={device} name={name} part={part} live={live} />
			))}
		</Section>
	);
};
