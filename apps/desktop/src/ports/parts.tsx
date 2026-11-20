// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pieces the ports popover and the port prompts share: the section
// with its heading, a row's text buttons, the line that says why an action
// failed, and the row for a port that listens with no forward.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import type { ReactNode } from "react";

import type { ListeningPort } from "@maxhq/shared/bindings";
import type { PortActions } from "@maxhq/shared/ports/actions";
import { cn } from "@/lib/utils";

export const TEXT_BUTTON =
	"inline-flex h-6 cursor-pointer items-center rounded-[5px] border border-input bg-secondary px-2 text-[11.5px] text-foreground outline-none hover:bg-hover-card focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-60";

/** A text button with no border, for the quieter action beside one. */
export const GHOST_TEXT_BUTTON =
	"inline-flex h-6 cursor-pointer items-center rounded-[5px] px-2 text-[11.5px] text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

/** Why a row's last action failed, across the whole row. */
export const Failure = ({ text }: { text: string | null }) =>
	text === null ? null : (
		<p role="alert" className="col-span-full text-[11.5px] text-status-error-text">
			{text}
		</p>
	);

/** A titled list in the popover. */
export const Section = ({ title, children }: { title: string; children: ReactNode }) => (
	<section aria-label={title} className="border-t border-secondary py-1.5">
		<h3 className="px-3.5 pt-1 pb-1 text-[10.5px] font-semibold tracking-[0.04em] text-muted-foreground uppercase">
			{title}
		</h3>
		<ul>{children}</ul>
	</section>
);

/** The process that holds a port, with its command on hover. */
export const Process = ({ port, quiet }: { port: ListeningPort; quiet?: boolean }) => {
	const { t } = useI18n();
	return (
		<span className="flex min-w-0 items-baseline gap-1.5" title={port.process?.command}>
			<span
				className={cn("shrink-0", quiet ? "text-secondary-foreground" : "text-foreground")}
			>
				{port.process?.name ?? t("Unknown process")}
			</span>
			{port.process ? (
				<span className="truncate font-mono text-[11px] text-muted-foreground">
					{port.process.command}
				</span>
			) : null}
		</span>
	);
};

export const PortNumber = ({ port }: { port: ListeningPort }) => {
	useI18n();
	return (
		<span className="font-mono text-[12px] text-foreground tabular-nums">
			{formatNumber(port.port, { useGrouping: false })}
		</span>
	);
};

/**
 * A port that listens with no forward: Forward for one on another device
 * that takes forwards now, Open for one on this Mac, and neither on a
 * device that can't take a forward. Dismiss follows when `onDismiss` is set.
 */
export const ListeningRow = ({
	port,
	remote,
	forwards,
	quiet,
	actions,
	onDismiss,
}: {
	port: ListeningPort;
	remote: boolean;
	forwards: boolean;
	quiet?: boolean;
	actions: PortActions;
	onDismiss?: () => void;
}) => {
	const { t } = useI18n();
	const number = formatNumber(port.port, { useGrouping: false });
	return (
		<li
			className={cn(
				"grid items-center gap-x-2 gap-y-0.5 px-3.5 py-1.5 text-[12px]",
				onDismiss
					? "grid-cols-[46px_minmax(0,1fr)_auto_auto]"
					: "grid-cols-[46px_minmax(0,1fr)_auto]"
			)}
		>
			<PortNumber port={port} />
			<Process port={port} quiet={quiet} />
			{remote && !forwards ? null : remote ? (
				<button
					type="button"
					aria-label={t("Forward port {{port}}", { port: number })}
					className={TEXT_BUTTON}
					disabled={actions.busy(port.id)}
					onClick={() => actions.forward(port)}
				>
					{t("Forward")}
				</button>
			) : (
				<button
					type="button"
					aria-label={t("Open localhost:{{port}} in the browser", { port: number })}
					className={TEXT_BUTTON}
					onClick={() => actions.open(port, port.port)}
				>
					{t("Open")}
				</button>
			)}
			{onDismiss ? (
				<button
					type="button"
					aria-label={t("Dismiss port {{port}}", { port: number })}
					className={GHOST_TEXT_BUTTON}
					onClick={onDismiss}
				>
					{t("Dismiss")}
				</button>
			) : null}
			<Failure text={actions.failure(port.id)} />
		</li>
	);
};
