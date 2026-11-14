// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES. The options are
// this Mac's sidebar settings rather than each server's, so a save is local
// and there is no draft to reconcile with what echoes back. The pure helpers
// moved to @maxhq/shared/view/snooze, a plain text field stands in for the
// number field, and the parts take MaxHQ's look.

import { Select } from "@base-ui/react/select";
import { Check, ChevronDown, Plus, X } from "lucide-react";
import { type FocusEvent, type KeyboardEvent, useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { setPrefs, usePrefs } from "@/sidebar/prefs";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatClock } from "@maxhq/shared/status/format";
import {
	DEFAULT_SNOOZE_PRESETS,
	MAX_SNOOZE_PRESET_MINUTES,
	MAX_SNOOZE_PRESETS,
	nextSnoozeCandidate,
	sameSnoozePresets,
	SNOOZE_PRESET_DAYS,
	SNOOZE_UNIT_MINUTES,
	type SnoozeKindMemory,
	snoozeMenuLine,
	snoozePresetId,
	type SnoozePresetSetting,
	snoozeRowNotes,
	type SnoozeUnit,
	type SnoozeWhen,
	snoozeWhenOf,
	sortSnoozeRows,
	splitSnoozeMinutes,
	withSnoozeWhen,
} from "@maxhq/shared/view/snooze";

import { FIELD, SECONDARY } from "./controls";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** One choice in a picker. */
export type Choice<T extends string> = { readonly value: T; readonly label: string };

const WHEN_LABELS: Record<SnoozeWhen, string> = {
	after: "In",
	today: "Today",
	tomorrow: "Tomorrow",
	monday: "Monday",
	tuesday: "Tuesday",
	wednesday: "Wednesday",
	thursday: "Thursday",
	friday: "Friday",
	saturday: "Saturday",
	sunday: "Sunday",
};

const whenChoices = (): readonly Choice<SnoozeWhen>[] =>
	(["after", ...SNOOZE_PRESET_DAYS] as const).map((value) => ({
		value,
		label: translate(WHEN_LABELS[value]),
	}));

const UNITS: readonly SnoozeUnit[] = ["minutes", "hours", "days"];
const ONE_UNIT: Record<SnoozeUnit, string> = { minutes: "minute", hours: "hour", days: "day" };

const formatTime = (date: Date): string => formatClock(date.getTime());

// A small picker that shows only its pick, as the row reads like a sentence.
// The default editor setting uses it too.
export const Pick = <T extends string>({
	label,
	value,
	options,
	onChange,
	className,
}: {
	label: string;
	value: T;
	options: readonly Choice<T>[];
	onChange: (value: T) => void;
	className?: string;
}) => {
	useI18n();
	const shown = options.find((o) => o.value === value)?.label ?? "";
	return (
		<Select.Root
			value={value}
			onValueChange={(next) => {
				const found = options.find((o) => o.value === next);
				if (found) onChange(found.value);
			}}
		>
			<Select.Trigger
				// A combobox takes no name from its text, so it says both here.
				aria-label={`${label}, ${shown}`}
				className={cn(
					"inline-flex h-7 min-w-0 items-center justify-between gap-1.5 rounded-[7px] border border-input bg-secondary pr-[7px] pl-2.5 text-xs outline-none",
					"hover:border-hover-card-border focus-visible:border-primary focus-visible:shadow-focus",
					"data-popup-open:border-primary data-popup-open:shadow-focus",
					className
				)}
			>
				<Select.Value className="min-w-0 truncate font-[550] text-foreground">
					{() => shown}
				</Select.Value>
				<ChevronDown
					aria-hidden
					size={12}
					strokeWidth={2.4}
					className="text-muted-foreground"
				/>
			</Select.Trigger>
			<Select.Portal>
				<Select.Positioner
					data-snooze-picker=""
					sideOffset={6}
					align="start"
					alignItemWithTrigger={false}
					className="z-[140] outline-none"
				>
					<Select.Popup className="max-h-[min(320px,var(--available-height))] min-w-[var(--anchor-width)] overflow-y-auto rounded-[10px] border border-input bg-popover p-[5px] text-foreground shadow-menu outline-none">
						<Select.List className="flex flex-col">
							{options.map((option) => (
								<Select.Item
									key={option.value}
									value={option.value}
									label={option.label}
									className="flex cursor-default items-center gap-2.5 rounded-md px-2 py-[6px] text-[12.5px] outline-none select-none data-highlighted:bg-accent data-selected:bg-primary/12"
								>
									<Select.ItemText className="flex-1 truncate">
										{option.label}
									</Select.ItemText>
									<Select.ItemIndicator>
										<Check
											aria-hidden
											size={14}
											strokeWidth={2.6}
											className="text-primary"
										/>
									</Select.ItemIndicator>
								</Select.Item>
							))}
						</Select.List>
					</Select.Popup>
				</Select.Positioner>
			</Select.Portal>
		</Select.Root>
	);
};

const DurationFields = ({
	minutes,
	onChange,
}: {
	minutes: number;
	onChange: (minutes: number) => void;
}) => {
	useI18n();
	// The chosen unit stays put, so 59 to 60 minutes doesn't jump to hours.
	const [chosen, setChosen] = useState(() => splitSnoozeMinutes(minutes).unit);
	const unit =
		minutes % SNOOZE_UNIT_MINUTES[chosen] === 0 ? chosen : splitSnoozeMinutes(minutes).unit;
	const amount = minutes / SNOOZE_UNIT_MINUTES[unit];
	const most = Math.floor(MAX_SNOOZE_PRESET_MINUTES / SNOOZE_UNIT_MINUTES[unit]);
	// Typing stays here until the field commits on blur or Enter, so a
	// half-typed amount never reaches the menu.
	const from = `${minutes}:${unit}`;
	const [typed, setTyped] = useState<{ text: string; from: string } | null>(null);
	const draft = typed && typed.from === from ? typed.text : null;
	const commit = () => {
		setTyped(null);
		// Anything but a whole number puts the amount back.
		const text = draft?.trim() ?? "";
		if (!/^\d+$/.test(text)) return;
		const next = Math.min(most, Math.max(1, Number(text)));
		if (next !== amount) onChange(next * SNOOZE_UNIT_MINUTES[unit]);
	};
	const names = amount === 1 ? ONE_UNIT : null;
	return (
		<>
			<input
				aria-label={translate("Amount")}
				inputMode="numeric"
				value={draft ?? String(amount)}
				onChange={(e) => setTyped({ text: e.target.value, from })}
				onBlur={commit}
				onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
					if (e.key === "Enter") commit();
				}}
				spellCheck={false}
				autoComplete="off"
				className={cn(FIELD, "h-7 w-[64px] text-right tabular-nums")}
			/>
			<Pick
				label={translate("Unit")}
				value={unit}
				options={UNITS.map((value) => ({
					value,
					label: translate(names?.[value] ?? value),
				}))}
				onChange={(next) => {
					setChosen(next);
					onChange(
						Math.min(amount * SNOOZE_UNIT_MINUTES[next], MAX_SNOOZE_PRESET_MINUTES)
					);
				}}
				className="w-[96px]"
			/>
		</>
	);
};

const PresetFields = ({
	preset,
	onChange,
}: {
	preset: SnoozePresetSetting;
	onChange: (preset: SnoozePresetSetting) => void;
}) => {
	useI18n();
	// Switching between a duration and a set time keeps the other kind's last
	// value, so a slip on "When" can be undone without retyping it.
	const [memory, setMemory] = useState<SnoozeKindMemory>({ minutes: 60, time: "09:00" });
	return (
		<span className="flex items-center gap-2">
			<Pick
				label={translate("When")}
				value={snoozeWhenOf(preset)}
				options={whenChoices()}
				onChange={(when) => {
					const next = withSnoozeWhen(preset, when, memory);
					if (next.kind !== preset.kind) {
						setMemory(
							preset.kind === "after"
								? { ...memory, minutes: preset.minutes }
								: { ...memory, time: preset.time }
						);
					}
					onChange(next);
				}}
				className="w-[112px]"
			/>
			{preset.kind === "after" ? (
				<DurationFields
					minutes={preset.minutes}
					onChange={(minutes) => onChange({ ...preset, minutes })}
				/>
			) : (
				<>
					<span className="text-xs text-muted-foreground">{translate("at")}</span>
					<input
						type="time"
						aria-label={translate("Time")}
						value={preset.time}
						onChange={(e) => {
							if (/^\d{2}:\d{2}$/.test(e.target.value)) {
								onChange({ ...preset, time: e.target.value });
							}
						}}
						className={cn(FIELD, "h-7 w-[104px] tabular-nums")}
					/>
				</>
			)}
		</span>
	);
};

/**
 * Edits the snooze menu's options. Rows sort by wake time like the menu,
 * but only when focus leaves the list, so a row never moves under the
 * pointer mid-edit.
 */
export const SnoozePresetsEditor = () => {
	useI18n();
	const services = useServices();
	const saved = usePrefs((p) => p.snoozePresets);
	const now = new Date(useNow(60_000));
	// Row keys survive removals and sorts, so a row's half-typed field stays
	// with it. One state, so a sort in the same event as an edit applies on top.
	const [rows, setRows] = useState(() =>
		sortSnoozeRows(
			saved,
			saved.map((_, i) => i),
			now
		)
	);
	const { presets, keys } = rows;
	const [nextKey, setNextKey] = useState(saved.length);
	const [seen, setSeen] = useState(saved);
	const freshKeys = (count: number) => {
		setNextKey(nextKey + count);
		return Array.from({ length: count }, (_, i) => nextKey + i);
	};
	// Another window saved a different list, so show that one.
	if (seen !== saved) {
		setSeen(saved);
		if (!sameSnoozePresets(saved, presets)) {
			setRows(sortSnoozeRows(saved, freshKeys(saved.length), now));
		}
	}

	const save = (next: readonly SnoozePresetSetting[], nextKeys: readonly number[] = keys) => {
		setRows({ presets: next, keys: nextKeys });
		setPrefs(services, { snoozePresets: next });
	};
	const replaceAt = (index: number, next: SnoozePresetSetting) => {
		if (snoozePresetId(next) === snoozePresetId(presets[index]!)) return;
		save(presets.map((preset, i) => (i === index ? next : preset)));
	};
	const settle = (e: FocusEvent<HTMLDivElement>) => {
		const list = e.currentTarget;
		const next = e.relatedTarget;
		if (list.contains(next)) return;
		// A picker's options open outside the list, so moving into them isn't leaving it.
		if (next instanceof Element && next.closest("[data-snooze-picker]")) return;
		// Waits for the amount field's own blur to save first.
		queueMicrotask(() => {
			if (!list.contains(document.activeElement)) {
				setRows((current) => sortSnoozeRows(current.presets, current.keys, now));
			}
		});
	};

	const notes = snoozeRowNotes(presets, now, formatTime);
	const full = presets.length >= MAX_SNOOZE_PRESETS;
	return (
		<div className="flex flex-col gap-2 px-4 py-3" onBlur={settle}>
			<p className="text-xs text-muted-foreground">
				{translate(
					"Choices in every snooze menu, listed soonest first. Weekdays mean the next one after today."
				)}
			</p>
			{presets.map((preset, i) => {
				const note = notes[i]!;
				return (
					<div
						key={keys[i]}
						role="group"
						aria-label={translate("Snooze option {{value1}}", { value1: i + 1 })}
						className="flex flex-wrap items-center gap-2"
					>
						<PresetFields preset={preset} onChange={(next) => replaceAt(i, next)} />
						<span className="ml-auto flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
							{note.shown ? (
								<span className="truncate">
									<span className="text-foreground">{note.label}</span>
									<span className="ps-2 tabular-nums">{note.when}</span>
								</span>
							) : (
								<span className="truncate">{note.text}</span>
							)}
							<button
								type="button"
								aria-label={translate("Remove snooze option {{value1}}", {
									value1: i + 1,
								})}
								onClick={() =>
									save(
										presets.filter((_, at) => at !== i),
										keys.filter((_, at) => at !== i)
									)
								}
								className="grid size-[26px] place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
							>
								<X aria-hidden size={14} strokeWidth={2} />
							</button>
						</span>
					</div>
				);
			})}
			<p className="text-xs text-muted-foreground">
				{snoozeMenuLine(presets, now, formatTime)}
			</p>
			<div className="flex items-center gap-2">
				<button
					type="button"
					className={SECONDARY}
					disabled={full}
					onClick={() =>
						save([...presets, nextSnoozeCandidate(presets)], [...keys, ...freshKeys(1)])
					}
				>
					<Plus aria-hidden size={14} strokeWidth={2} />
					{translate("Add option")}
				</button>
				{full ? (
					<span className="text-xs text-muted-foreground">
						{translate("Up to ")}
						{MAX_SNOOZE_PRESETS} {translate(" options")}
					</span>
				) : null}
				{sameSnoozePresets(presets, DEFAULT_SNOOZE_PRESETS) ? null : (
					<button
						type="button"
						className={cn(SECONDARY, "ml-auto")}
						onClick={() => {
							const sorted = sortSnoozeRows(
								DEFAULT_SNOOZE_PRESETS,
								freshKeys(DEFAULT_SNOOZE_PRESETS.length),
								now
							);
							save(sorted.presets, sorted.keys);
						}}
					>
						{translate("Reset to defaults")}
					</button>
				)}
			</div>
		</div>
	);
};
