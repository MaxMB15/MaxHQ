// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A chip that names what it picks and shows the pick, and opens a list of
// options with a second line each, as on the selector board.

import { Select } from "@base-ui/react/select";
import { Fragment } from "react";

import { Bar } from "@/header/usage";
import { cn } from "@/lib/utils";
import { TONE } from "@/tasks/activity";
import { formatPercent } from "@maxhq/shared/status/format";
import { type WindowView, windowShort } from "@maxhq/shared/status/usage";

import type { Option } from "@maxhq/shared/new-agent/spec";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Chevron = () => {
	useI18n();
	return (
		<svg
			aria-hidden
			width={12}
			height={12}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2.4}
			strokeLinecap="round"
			strokeLinejoin="round"
			className="shrink-0 text-muted-foreground"
		>
			<path d="M6 9l6 6 6-6" />
		</svg>
	);
};

const Check = () => {
	useI18n();
	return (
		<svg
			aria-hidden
			width={14}
			height={14}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2.6}
			strokeLinecap="round"
			strokeLinejoin="round"
			className="shrink-0 text-primary"
		>
			<path d="M5 12.5l4.5 4.5L19 7.5" />
		</svg>
	);
};

// What a screen reader hears for one window, such as "5 hours, 62% used".
const windowText = (w: WindowView): string => {
	if (w.usedUp) return translate("{{label}}, at its limit", { label: w.label });
	if (w.percent === null) return translate("{{label}}, not checked yet", { label: w.label });
	return translate("{{label}}, {{percent}} used", {
		label: w.label,
		percent: formatPercent(w.percent),
	});
};

// A login's windows, one bar each, shortest first.
const Usage = ({ windows }: { windows: readonly WindowView[] }) => {
	useI18n();
	return (
		<span className="flex shrink-0 flex-col gap-[3px]">
			{windows.map((w) => (
				<span key={w.minutes} data-window={w.minutes}>
					<span
						aria-hidden
						className="grid grid-cols-[18px_44px_26px] items-center gap-x-1.5 text-[10.5px] text-muted-foreground"
					>
						<span>{windowShort(w.minutes)}</span>
						<Bar
							percent={w.percent}
							full={w.usedUp}
							className="h-1 rounded-[2px] bg-input"
						/>
						<span
							className={cn(
								"text-right tabular-nums",
								w.usedUp ? "text-status-busy" : "text-secondary-foreground"
							)}
						>
							{w.percent === null ? "" : formatPercent(w.percent)}
						</span>
					</span>
					<span className="sr-only">{windowText(w)}</span>
				</span>
			))}
		</span>
	);
};

/** One picker. */
export const Picker = ({
	label,
	value,
	options,
	onChange,
	disabled,
	wrapDetails,
	className,
}: {
	/** What it picks, shown before the pick. */
	label: string;
	value: string;
	options: readonly Option[];
	onChange: (value: string) => void;
	disabled?: boolean;
	/** Lets each option's second line wrap, for text too long for one line. */
	wrapDetails?: boolean;
	className?: string;
}) => {
	useI18n();
	const shown = options.find((o) => o.value === value)?.label ?? translate("None");
	return (
		<Select.Root
			value={value}
			onValueChange={(next) => {
				if (typeof next === "string") onChange(next);
			}}
			disabled={disabled}
		>
			<Select.Trigger
				// A combobox takes no name from its text, so it says both here.
				aria-label={`${label}, ${shown}`}
				className={cn(
					"inline-flex h-7 max-w-full min-w-0 items-center gap-1.5 rounded-[7px] border border-input bg-secondary pr-[9px] pl-2.5 text-xs outline-none",
					"enabled:hover:border-hover-card-border focus-visible:border-primary focus-visible:shadow-focus",
					"data-popup-open:border-primary data-popup-open:shadow-focus disabled:opacity-60",
					className
				)}
			>
				<span className="shrink-0 text-muted-foreground">{label}</span>
				<Select.Value className="min-w-0 truncate font-[550] text-foreground">
					{() => shown}
				</Select.Value>
				<Chevron />
			</Select.Trigger>
			<Select.Portal>
				<Select.Positioner
					sideOffset={6}
					align="start"
					alignItemWithTrigger={false}
					className="z-[140] outline-none"
				>
					<Select.Popup className="max-h-[min(360px,var(--available-height))] min-w-[max(var(--anchor-width),260px)] max-w-[360px] overflow-y-auto rounded-[10px] border border-input bg-popover p-[5px] text-foreground shadow-menu outline-none">
						<Select.List className="flex flex-col">
							{options.map((option, i) => {
								const heading =
									option.group !== undefined &&
									option.group !== options[i - 1]?.group;
								// A row with no group after grouped ones, such as Set up local models, stands apart.
								const apart =
									option.group === undefined &&
									options[i - 1]?.group !== undefined;
								return (
									<Fragment key={option.value}>
										{apart ? (
											<div
												role="separator"
												className="mx-1 my-1 h-px bg-border"
											/>
										) : null}
										{heading ? (
											<div className="px-2 pt-2 pb-1 text-[11px] text-muted-foreground">
												{option.group}
											</div>
										) : null}
										<Select.Item
											value={option.value}
											label={option.label}
											disabled={option.disabled}
											className="flex cursor-default items-center gap-2.5 rounded-md px-2 py-[7px] outline-none select-none data-disabled:opacity-55 data-highlighted:bg-accent data-selected:bg-primary/12"
										>
											<span className="flex min-w-0 flex-1 flex-col gap-px">
												<Select.ItemText className="truncate text-[12.5px] font-[550]">
													{option.label}
												</Select.ItemText>
												{option.detail ? (
													<span
														className={cn(
															"text-[11px]",
															!wrapDetails && "truncate",
															option.warn
																? "text-status-busy-text"
																: "text-muted-foreground"
														)}
													>
														{option.detail}
													</span>
												) : null}
											</span>
											{option.usage && option.usage.length > 0 ? (
												<Usage windows={option.usage} />
											) : null}
											{option.tag ? (
												<span
													data-tone={option.tone}
													className={cn(
														"h-[18px] shrink-0 rounded-[4px] px-1.5 text-[10.5px] leading-[18px]",
														option.tone
															? TONE[option.tone]
															: "bg-border text-secondary-foreground"
													)}
												>
													{option.tag}
												</span>
											) : null}
											<Select.ItemIndicator>
												<Check />
											</Select.ItemIndicator>
										</Select.Item>
									</Fragment>
								);
							})}
						</Select.List>
					</Select.Popup>
				</Select.Positioner>
			</Select.Portal>
		</Select.Root>
	);
};
