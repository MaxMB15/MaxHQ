// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES. One request store
// for each window instead of a global one, the answer is a time rather than a
// choice, native date, time and number fields stand in for the calendar and
// number field, and the parts take MaxHQ's look.

import { t, useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { Settings2 } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { FIELD, Label, PRIMARY, SECONDARY } from "@/settings/controls";
import { openSettings } from "@/settings/store";
import type { Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	type CustomSnoozeInput,
	localSnoozeDate,
	localSnoozeTime,
	resolveCustomSnooze,
} from "@maxhq/shared/view/snooze";

type Request = { readonly resolve: (until: Timestamp | null) => void };
type RequestState = { readonly request: Request | null };

const stores = new WeakMap<Services, StoreApi<RequestState>>();

const requestsOf = (services: Services): StoreApi<RequestState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<RequestState>(() => ({ request: null }));
		stores.set(services, store);
	}
	return store;
};

/**
 * Asks when to wake, in the custom snooze dialog. Resolves with the time,
 * or null when you cancel. A new request cancels the one before.
 */
export const requestCustomSnooze = (services: Services): Promise<Timestamp | null> => {
	const store = requestsOf(services);
	store.getState().request?.resolve(null);
	return new Promise((resolve) => store.setState({ request: { resolve } }));
};

const finish = (services: Services, until: Timestamp | null) => {
	const store = requestsOf(services);
	const request = store.getState().request;
	store.setState({ request: null });
	request?.resolve(until);
};

const UNITS = [
	{
		value: "minutes",
		get label() {
			return t("Minutes");
		},
	},
	{
		value: "hours",
		get label() {
			return t("Hours");
		},
	},
	{
		value: "days",
		get label() {
			return t("Days");
		},
	},
] as const;

type Unit = (typeof UNITS)[number]["value"];

const isUnit = (value: string): value is Unit => UNITS.some((unit) => unit.value === value);

const MODES = [
	{
		mode: "date",
		get label() {
			return t("Date and time");
		},
	},
	{
		mode: "duration",
		get label() {
			return t("Duration");
		},
	},
] as const;

const Form = () => {
	const { t } = useI18n();

	const services = useServices();
	const id = useId();
	const [initial] = useState(() => new Date(Date.now() + 3_600_000));
	// Follows the clock so a dialog left open past midnight stops offering yesterday.
	const today = localSnoozeDate(new Date(useNow(60_000)));
	const [mode, setMode] = useState<CustomSnoozeInput["mode"]>("date");
	const [date, setDate] = useState(localSnoozeDate(initial));
	const [time, setTime] = useState(localSnoozeTime(initial));
	const [amount, setAmount] = useState("2");
	const [unit, setUnit] = useState<Unit>("hours");
	const [error, setError] = useState<string | null>(null);
	const input: CustomSnoozeInput =
		mode === "date" ? { mode, date, time } : { mode, amount, unit };

	return (
		<form
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				const until = resolveCustomSnooze(input, new Date());
				if (until === null) {
					setError(
						mode === "date"
							? t("Choose a valid date and time in the future.")
							: t("Enter a positive duration.")
					);
					return;
				}
				finish(services, until);
			}}
		>
			<div className="flex flex-col gap-1">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{t("Custom snooze")}
				</Dialog.Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{t("Choose when snoozed agents return to your inbox.")}
				</Dialog.Description>
			</div>
			<div
				role="group"
				aria-label={t("Schedule type")}
				className="flex gap-0.5 rounded-[7px] border border-sidebar-border bg-background p-0.5"
			>
				{MODES.map((m) => (
					<button
						key={m.mode}
						type="button"
						aria-pressed={mode === m.mode}
						onClick={() => {
							setMode(m.mode);
							setError(null);
						}}
						className={cn(
							"h-6 flex-1 rounded-[5px] text-xs",
							mode === m.mode
								? "bg-border font-[550] text-foreground"
								: "text-muted-foreground hover:text-secondary-foreground"
						)}
					>
						{m.label}
					</button>
				))}
			</div>
			{mode === "date" ? (
				<div className="grid grid-cols-2 gap-3">
					<label className="flex min-w-0 flex-col gap-1.5" htmlFor={`${id}-date`}>
						<Label>{t("Date")}</Label>
						<input
							id={`${id}-date`}
							type="date"
							required
							min={today}
							value={date}
							onChange={(e) => {
								setDate(e.target.value);
								setError(null);
							}}
							className={FIELD}
						/>
					</label>
					<label className="flex min-w-0 flex-col gap-1.5" htmlFor={`${id}-time`}>
						<Label>{t("Time")}</Label>
						<input
							id={`${id}-time`}
							type="time"
							required
							value={time}
							onChange={(e) => {
								setTime(e.target.value);
								setError(null);
							}}
							className={FIELD}
						/>
					</label>
				</div>
			) : (
				<div className="grid grid-cols-2 gap-3">
					<label className="flex min-w-0 flex-col gap-1.5" htmlFor={`${id}-amount`}>
						<Label>{t("Snooze for")}</Label>
						<input
							id={`${id}-amount`}
							type="number"
							required
							min={0}
							step="any"
							value={amount}
							onChange={(e) => {
								setAmount(e.target.value);
								setError(null);
							}}
							className={cn(FIELD, "tabular-nums")}
						/>
					</label>
					<label className="flex min-w-0 flex-col gap-1.5" htmlFor={`${id}-unit`}>
						<Label>{t("Unit")}</Label>
						<select
							id={`${id}-unit`}
							value={unit}
							onChange={(e) => {
								if (isUnit(e.target.value)) setUnit(e.target.value);
								setError(null);
							}}
							className={FIELD}
						>
							{UNITS.map((u) => (
								<option key={u.value} value={u.value}>
									{u.label}
								</option>
							))}
						</select>
					</label>
				</div>
			)}
			{error ? (
				<p role="alert" className="text-[11.5px] text-status-error-text">
					{error}
				</p>
			) : null}
			<div className="flex items-center gap-2">
				<button
					type="button"
					onClick={() => {
						finish(services, null);
						openSettings(services, { kind: "sidebar" });
					}}
					className="mr-auto inline-flex h-[30px] items-center gap-1.5 rounded-[7px] px-2 text-[12.5px] text-muted-foreground hover:bg-secondary hover:text-secondary-foreground"
				>
					<Settings2 aria-hidden size={14} strokeWidth={1.8} />
					{t("Edit snooze options")}
				</button>
				<button type="button" onClick={() => finish(services, null)} className={SECONDARY}>
					{t("Cancel")}
				</button>
				<button type="submit" className={PRIMARY}>
					{t("Snooze")}
				</button>
			</div>
		</form>
	);
};

/** The custom snooze dialog, shown while a request waits for an answer. */
export const CustomSnoozeHost = () => {
	useI18n();

	const services = useServices();
	const request = useStore(requestsOf(services), (s) => s.request);
	// A request left open when the host goes away answers as cancelled.
	useEffect(() => () => finish(services, null), [services]);
	return (
		<Dialog.Root
			open={request !== null}
			onOpenChange={(open) => {
				if (!open) finish(services, null);
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-[16vh] left-1/2 z-[121] w-[min(400px,calc(100vw-32px))] -translate-x-1/2 rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					{request ? <Form /> : null}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
