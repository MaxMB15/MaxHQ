// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One lease, opened from a device's leases or its holder's header: who
// holds it and how long it has, the test machine's screen, taken again
// every 2 seconds while the view is open and the lease ready, the commands
// it last ran, and End lease.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useEffect, useState } from "react";

import { useServices, useView } from "@/app/services";
import { ConfirmButton, SECONDARY, StatusNote } from "@/settings/controls";
import { failureOf } from "@/settings/result";
import type { LeaseId, TestLease } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatWhen } from "@maxhq/shared/status/format";
import { commandOutcome, LEASE_SCREEN_EVERY, leaseHeld } from "@maxhq/shared/test-hosts/device";
import {
	LEASE_END,
	leaseCommandLine,
	type LeaseScreen,
	leaseScreenOf,
} from "@maxhq/shared/test-hosts/format";
import { kindLabel } from "@maxhq/shared/test-hosts/hosts";

import { useCall } from "./places";

/** How often an open lease view takes the screen again. */
export const SCREEN_EVERY = LEASE_SCREEN_EVERY;

const Screen = ({ lease }: { lease: TestLease }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(SCREEN_EVERY);
	const [screen, setScreen] = useState<LeaseScreen | null>(null);
	const [problem, setProblem] = useState<string | null>(null);
	const ready = lease.state === "ready";
	useEffect(() => {
		if (!ready) return;
		let gone = false;
		const take = async () => {
			try {
				const result = await source.readLeaseScreen(lease.device, lease.id);
				if (gone) return;
				const shot = leaseScreenOf(result);
				if (shot) setScreen(shot);
				setProblem(shot ? null : (failureOf(result) ?? t("It sent no screen.")));
			} catch (error) {
				console.warn("Couldn't take the lease's screen", error);
				if (!gone) setProblem(t("Couldn't reach it. Trying again."));
			}
		};
		void take();
		const timer = setInterval(() => void take(), SCREEN_EVERY);
		return () => {
			gone = true;
			clearInterval(timer);
		};
	}, [source, lease.device, lease.id, ready, t]);
	return (
		<div className="flex flex-col gap-1.5">
			<div className="grid min-h-[220px] place-items-center overflow-hidden rounded-lg border border-sidebar-border bg-subpanel">
				{screen ? (
					<img
						src={screen.src}
						alt={`${kindLabel(lease.kind)} screen`}
						className="max-h-[420px] max-w-full object-contain"
					/>
				) : (
					<span className="px-4 text-center text-xs text-muted-foreground">
						{ready
							? t("Taking the screen.")
							: t("The screen shows once the lease is ready.")}
					</span>
				)}
			</div>
			<span className="text-[11.5px] text-muted-foreground">
				{problem ??
					(screen
						? t("Taken at {{value1}}", { value1: formatWhen(screen.at, now) })
						: "")}
			</span>
		</div>
	);
};

const Commands = ({ lease }: { lease: TestLease }) => {
	const { t } = useI18n();

	const now = useNow(30_000);
	const recent = lease.recent ?? [];
	return (
		<div className="flex flex-col gap-1">
			<h3 className="text-xs font-[650] text-muted-foreground">{t("Last commands")}</h3>
			{recent.length === 0 ? (
				<p className="text-xs text-muted-foreground">{t("No commands yet.")}</p>
			) : (
				<ul aria-label={t("Last commands")} className="flex flex-col gap-1">
					{[...recent].reverse().map((command) => (
						<li
							key={`${command.at}-${command.command.join(" ")}`}
							className="flex items-baseline gap-2 text-xs"
						>
							<code className="min-w-0 flex-1 truncate font-mono text-terminal">
								{leaseCommandLine(command)}
							</code>
							<span className="shrink-0 text-muted-foreground">
								{commandOutcome(command)}, {formatWhen(command.at, now)}
							</span>
						</li>
					))}
				</ul>
			)}
		</div>
	);
};

const Body = ({ lease }: { lease: TestLease }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const { source } = useServices();
	const now = useNow(30_000);
	const { note, busy, call } = useCall();
	return (
		<>
			<div className="flex items-center gap-3">
				<Dialog.Title className="min-w-0 flex-1 truncate text-[15px] font-[650] text-foreground-strong">
					{t("{{value2}} lease", { value2: kindLabel(lease.kind) })}
				</Dialog.Title>
				<Dialog.Close className={SECONDARY}>{t("Close")}</Dialog.Close>
			</div>
			<p className="text-[12.5px] text-secondary-foreground">{leaseHeld(view, lease, now)}</p>
			<Screen lease={lease} />
			<Commands lease={lease} />
			<div className="flex items-center justify-end gap-2">
				<StatusNote note={note} />
				<ConfirmButton
					label={t("End lease")}
					question={t("Its holder loses the test machine.")}
					confirm={t("End lease")}
					disabled={busy}
					onConfirm={() =>
						void call(t("Ending"), () => source.endLease(lease.device, lease.id))
					}
				/>
			</div>
		</>
	);
};

/** A lease's view, open while `lease` names one. */
export const LeaseView = ({ lease, onClose }: { lease: LeaseId | null; onClose: () => void }) => {
	const { t } = useI18n();

	const found = useView((s) => (lease ? s.view.tables.test_lease.get(lease) : undefined));
	return (
		<Dialog.Root
			open={lease !== null}
			onOpenChange={(next) => {
				if (!next) onClose();
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-1/2 left-1/2 z-[121] flex max-h-[calc(100vh-64px)] w-[min(600px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 overflow-y-auto rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					{found && found.state !== "ended" ? (
						<Body lease={found} />
					) : (
						<>
							<div className="flex items-center gap-3">
								<Dialog.Title className="min-w-0 flex-1 truncate text-[15px] font-[650] text-foreground-strong">
									{found
										? t("{{value1}} lease", { value1: kindLabel(found.kind) })
										: t("Lease")}
								</Dialog.Title>
								<Dialog.Close className={SECONDARY}>{t("Close")}</Dialog.Close>
							</div>
							<p className="text-[12.5px] text-muted-foreground">
								{t("This lease has ended.{{value2}}", {
									value2: found?.ended ? ` ${LEASE_END[found.ended]}` : "",
								})}
							</p>
						</>
					)}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
