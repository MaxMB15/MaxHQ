// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A chip that names what it picks and shows the pick, and opens a list of
// options with a second line each, as on the selector board. A model
// picker with a long list, such as an OpenRouter login's, opens a list
// with a search field on top instead.

import { Combobox } from "@base-ui/react/combobox";
import { Select } from "@base-ui/react/select";
import { Fragment, type ReactNode, useMemo, useState } from "react";

import { Bar } from "@/header/usage";
import { cn } from "@/lib/utils";
import { TONE } from "@/tasks/activity";
import { showsModelSearch } from "@maxhq/shared/agent-view/model-search";
import { filterModelOptions, modelOptionCount } from "@maxhq/shared/local-models/picker";
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

/** What both kinds of picker take. */
type PickerProps = {
	/** What it picks, shown before the pick. */
	label: string;
	value: string;
	options: readonly Option[];
	onChange: (value: string) => void;
	disabled?: boolean;
	/** Lets each option's second line wrap, for text too long for one line. */
	wrapDetails?: boolean;
	className?: string;
};

const TRIGGER = [
	"inline-flex h-7 max-w-full min-w-0 items-center gap-1.5 rounded-[7px] border border-input bg-secondary pr-[9px] pl-2.5 text-xs outline-none",
	"enabled:hover:border-hover-card-border focus-visible:border-primary focus-visible:shadow-focus",
	"data-popup-open:border-primary data-popup-open:shadow-focus disabled:opacity-60",
];

const POPUP =
	"rounded-[10px] border border-input bg-popover p-[5px] text-foreground shadow-menu outline-none";

const ITEM =
	"flex cursor-default items-center gap-2.5 rounded-md px-2 py-[7px] outline-none select-none data-disabled:opacity-55 data-highlighted:bg-accent data-selected:bg-primary/12";

/** The name of the option the picker shows. */
const shownLabel = (options: readonly Option[], value: string): string =>
	options.find((o) => o.value === value)?.label ?? translate("None");

/**
 * The options, each under its group's heading where the group changes.
 * `item` draws one option's row, around what `OptionBody` draws.
 */
const OptionRows = ({
	options,
	item,
}: {
	options: readonly Option[];
	item: (option: Option, index: number) => ReactNode;
}) => {
	useI18n();
	return options.map((option, i) => {
		const heading = option.group !== undefined && option.group !== options[i - 1]?.group;
		// A row with no group after grouped ones, such as Set up local models, stands apart.
		const apart = option.group === undefined && options[i - 1]?.group !== undefined;
		return (
			<Fragment key={option.value}>
				{apart ? <div role="separator" className="mx-1 my-1 h-px bg-border" /> : null}
				{heading ? (
					<div className="px-2 pt-2 pb-1 text-[11px] text-muted-foreground">
						{option.group}
					</div>
				) : null}
				{item(option, i)}
			</Fragment>
		);
	});
};

/** One option's row: its name, given as `name`, its second line, usage and tag. */
const OptionBody = ({
	option,
	name,
	wrapDetails,
}: {
	option: Option;
	name: ReactNode;
	wrapDetails: boolean | undefined;
}) => {
	useI18n();
	return (
		<>
			<span className="flex min-w-0 flex-1 flex-col gap-px">
				{name}
				{option.detail ? (
					<span
						className={cn(
							"text-[11px]",
							!wrapDetails && "truncate",
							option.warn ? "text-status-busy-text" : "text-muted-foreground"
						)}
					>
						{option.detail}
					</span>
				) : null}
			</span>
			{option.usage && option.usage.length > 0 ? <Usage windows={option.usage} /> : null}
			{option.tag ? (
				<span
					data-tone={option.tone}
					className={cn(
						"h-[18px] shrink-0 rounded-[4px] px-1.5 text-[10.5px] leading-[18px]",
						option.tone ? TONE[option.tone] : "bg-border text-secondary-foreground"
					)}
				>
					{option.tag}
				</span>
			) : null}
		</>
	);
};

const NAME = "truncate text-[12.5px] font-[550]";

/** One picker. */
export const Picker = ({
	label,
	value,
	options,
	onChange,
	disabled,
	wrapDetails,
	className,
	models = false,
}: PickerProps & {
	/**
	 * Whether it picks a model. Past a dozen models, as `showsModelSearch`
	 * says, its list gets a search field; see `ModelSearchPicker`.
	 */
	models?: boolean;
}) => {
	useI18n();
	if (models && showsModelSearch(modelOptionCount(options))) {
		return (
			<ModelSearchPicker
				label={label}
				value={value}
				options={options}
				onChange={onChange}
				disabled={disabled}
				wrapDetails={wrapDetails}
				className={className}
			/>
		);
	}
	const shown = shownLabel(options, value);
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
				className={cn(TRIGGER, className)}
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
					<Select.Popup
						className={cn(
							POPUP,
							"max-h-[min(360px,var(--available-height))] min-w-[max(var(--anchor-width),260px)] max-w-[360px] overflow-y-auto"
						)}
					>
						<Select.List className="flex flex-col">
							<OptionRows
								options={options}
								item={(option) => (
									<Select.Item
										value={option.value}
										label={option.label}
										disabled={option.disabled}
										className={ITEM}
									>
										<OptionBody
											option={option}
											wrapDetails={wrapDetails}
											name={
												<Select.ItemText className={NAME}>
													{option.label}
												</Select.ItemText>
											}
										/>
										<Select.ItemIndicator>
											<Check />
										</Select.ItemIndicator>
									</Select.Item>
								)}
							/>
						</Select.List>
					</Select.Popup>
				</Select.Positioner>
			</Select.Portal>
		</Select.Root>
	);
};

/**
 * A model picker whose list opens with a search field on top, which keeps
 * the rows whose name or model has every word typed, as
 * `filterModelOptions` says. A select can't hold a field, as its own
 * typeahead takes the keys, so this is a combobox with its field inside
 * the popup: the chip looks the same, typing goes to the field, the arrows
 * move through the rows while the field keeps focus, Enter picks, and Esc
 * closes the list without closing the form. Closing it empties the field.
 */
const ModelSearchPicker = ({
	label,
	value,
	options,
	onChange,
	disabled,
	wrapDetails,
	className,
}: PickerProps) => {
	useI18n();
	const [query, setQuery] = useState("");
	const items = useMemo(
		() =>
			Combobox.createItems(options, {
				getValue: (option: Option) => option.value,
				getLabel: (option: Option) => option.label,
			}),
		[options]
	);
	const found = useMemo(() => filterModelOptions(options, query), [options, query]);
	const none = modelOptionCount(found) === 0;
	const shown = shownLabel(options, value);
	return (
		<Combobox.Root
			items={items}
			filteredItems={found}
			filter={null}
			value={value}
			onValueChange={(next) => {
				if (typeof next === "string") onChange(next);
			}}
			inputValue={query}
			onInputValueChange={setQuery}
			onOpenChange={(open) => {
				if (!open) setQuery("");
			}}
			disabled={disabled}
		>
			<Combobox.Trigger aria-label={`${label}, ${shown}`} className={cn(TRIGGER, className)}>
				<span className="shrink-0 text-muted-foreground">{label}</span>
				<span className="min-w-0 truncate font-[550] text-foreground">{shown}</span>
				<Chevron />
			</Combobox.Trigger>
			<Combobox.Portal>
				<Combobox.Positioner sideOffset={6} align="start" className="z-[140] outline-none">
					{/* It keeps its size while you type, so the field stays put. */}
					<Combobox.Popup
						className={cn(
							POPUP,
							"flex h-[min(360px,var(--available-height))] w-[max(var(--anchor-width),360px)] max-w-[var(--available-width)] flex-col"
						)}
					>
						<Combobox.Input
							aria-label={translate("Search models")}
							placeholder={translate("Search models")}
							spellCheck={false}
							autoComplete="off"
							className="mb-[5px] h-7 w-full shrink-0 rounded-[6px] border border-input bg-card px-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground focus:border-ring"
						/>
						{none ? (
							<p
								role="status"
								className="px-2 py-3 text-[12px] text-muted-foreground"
							>
								{translate("No models match")}
							</p>
						) : null}
						<Combobox.List className="flex min-h-0 flex-1 flex-col overflow-y-auto">
							<OptionRows
								options={found}
								item={(option, index) => (
									<Combobox.Item
										value={option.value}
										index={index}
										disabled={option.disabled}
										className={ITEM}
									>
										<OptionBody
											option={option}
											wrapDetails={wrapDetails}
											name={<span className={NAME}>{option.label}</span>}
										/>
										<Combobox.ItemIndicator>
											<Check />
										</Combobox.ItemIndicator>
									</Combobox.Item>
								)}
							/>
						</Combobox.List>
					</Combobox.Popup>
				</Combobox.Positioner>
			</Combobox.Portal>
		</Combobox.Root>
	);
};
