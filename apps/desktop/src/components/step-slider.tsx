// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A horizontal slider that snaps to a few named stops, such as effort
// levels. Each stop's name sits under it, the current one emphasized, and
// the stop marked as the default says so. Clicking or dragging picks a
// stop, and the arrow keys, Home and End move the thumb. A pick applies at
// once; a drag shows where it goes and picks when it lets go.

import { type KeyboardEvent, type PointerEvent, type RefObject, useRef, useState } from "react";

import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { cn } from "@/lib/utils";

/** One stop on the slider. */
export type Stop = {
	readonly value: string;
	readonly label: string;
	/** Whether it's the one used when nothing is picked. */
	readonly isDefault?: boolean;
};

/** How wide each stop's column is, in pixels, so its name fits under it. */
const COLUMN = 58;

/** How far a key moves the thumb, or where to. */
const STEP: Record<string, (at: number, last: number) => number> = {
	ArrowLeft: (at) => at - 1,
	ArrowDown: (at) => at - 1,
	ArrowRight: (at) => at + 1,
	ArrowUp: (at) => at + 1,
	Home: () => 0,
	End: (_, last) => last,
};

/** Where a stop's center sits, as a share of the slider's width. */
const center = (index: number, count: number): string => `${((index + 0.5) / count) * 100}%`;

export const StepSlider = ({
	label,
	stops,
	value,
	onPick,
	onDone,
	disabled = false,
	thumbRef,
	className,
}: {
	/** What the slider sets, for screen readers. */
	label: string;
	stops: readonly Stop[];
	value: string;
	onPick: (value: string) => void;
	/** Called on Enter or Escape, to close what holds the slider. */
	onDone?: () => void;
	disabled?: boolean;
	/** The thumb, for whatever holds the slider to focus it. */
	thumbRef?: RefObject<HTMLDivElement | null>;
	className?: string;
}) => {
	useI18n();
	const track = useRef<HTMLDivElement>(null);
	const own = useRef<HTMLDivElement>(null);
	const thumb = thumbRef ?? own;
	// The stop picked here, until `value` moves away from what it was then,
	// so the thumb doesn't jump back while the pick is on its way.
	const [picked, setPicked] = useState<{ from: string; to: string } | null>(null);
	const [dragging, setDragging] = useState<number | null>(null);
	const shown = picked !== null && picked.from === value ? picked.to : value;
	const count = stops.length;
	const last = count - 1;
	const at =
		dragging ??
		Math.max(
			0,
			stops.findIndex((stop) => stop.value === shown)
		);
	const current = stops[at];
	const marked = stops.some((stop) => stop.isDefault);

	const pick = (index: number) => {
		const stop = stops[Math.min(Math.max(index, 0), last)];
		if (!stop || stop.value === shown) return;
		setPicked({ from: value, to: stop.value });
		onPick(stop.value);
	};

	/** The stop under a pointer at `x`. */
	const indexAt = (x: number): number => {
		const box = track.current?.getBoundingClientRect();
		if (!box || box.width <= 0) return at;
		const inside = Math.min(Math.max(x - box.left, 0), box.width - 1);
		return Math.floor((inside / box.width) * count);
	};

	const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
		if (disabled || event.button > 0) return;
		// Keep the focus on the thumb rather than the stop's name.
		event.preventDefault();
		event.currentTarget.setPointerCapture?.(event.pointerId);
		thumb.current?.focus();
		setDragging(indexAt(event.clientX));
	};
	const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
		if (dragging !== null) setDragging(indexAt(event.clientX));
	};
	const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
		if (dragging === null) return;
		setDragging(null);
		pick(indexAt(event.clientX));
	};

	const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
		const step = STEP[event.key];
		if (step && disabled) return;
		if (step) {
			event.preventDefault();
			pick(step(at, last));
		} else if ((event.key === "Enter" || event.key === "Escape") && onDone) {
			event.preventDefault();
			onDone();
		}
	};

	if (count === 0) return null;
	return (
		<div
			ref={track}
			className={cn(
				"relative cursor-pointer touch-none select-none",
				disabled && "pointer-events-none opacity-50",
				className
			)}
			style={{ width: count * COLUMN }}
			onPointerDown={onPointerDown}
			onPointerMove={onPointerMove}
			onPointerUp={onPointerUp}
			onPointerCancel={() => setDragging(null)}
		>
			<div className="relative h-5">
				<div
					className="absolute top-1/2 h-[3px] -translate-y-1/2 rounded-full bg-muted-foreground/25"
					style={{ left: center(0, count), right: center(0, count) }}
				/>
				<div
					className="absolute top-1/2 h-[3px] -translate-y-1/2 rounded-full bg-primary"
					style={{ left: center(0, count), width: `${(at / count) * 100}%` }}
				/>
				{stops.map((stop, index) => (
					<span
						key={stop.value}
						aria-hidden
						className={cn(
							"absolute top-1/2 size-[6px] -translate-x-1/2 -translate-y-1/2 rounded-full",
							index <= at ? "bg-primary" : "bg-muted-foreground/45"
						)}
						style={{ left: center(index, count) }}
					/>
				))}
				<div
					ref={thumb}
					role="slider"
					tabIndex={disabled ? -1 : 0}
					aria-label={label}
					aria-valuemin={0}
					aria-valuemax={last}
					aria-valuenow={at}
					aria-valuetext={
						current?.isDefault
							? translate("{{level}} (default)", { level: current.label })
							: current?.label
					}
					aria-disabled={disabled || undefined}
					aria-orientation="horizontal"
					className={cn(
						"absolute top-1/2 size-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-primary bg-background shadow-sm outline-none focus-visible:ring-2 focus-visible:ring-ring",
						dragging === null && "transition-[left] duration-100"
					)}
					style={{ left: center(at, count) }}
					onKeyDown={onKeyDown}
				/>
			</div>
			<div
				aria-hidden
				className="grid pt-1"
				style={{ gridTemplateColumns: `repeat(${count}, minmax(0, 1fr))` }}
			>
				{stops.map((stop, index) => (
					<div key={stop.value} className="flex flex-col items-center gap-0.5">
						<span
							className={cn(
								"text-[11px] leading-4 whitespace-nowrap",
								index === at
									? "font-semibold text-foreground"
									: "text-muted-foreground hover:text-foreground"
							)}
						>
							{stop.label}
						</span>
						{marked && (
							<span className="h-3 text-[10px] leading-3 text-muted-foreground">
								{stop.isDefault ? translate("Default") : ""}
							</span>
						)}
					</div>
				))}
			</div>
		</div>
	);
};
