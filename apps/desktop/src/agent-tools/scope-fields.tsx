// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The fields of a permission to run agents: the devices, vendors, accounts
// and places it covers, the test machines it may lease, whether agents in a
// container may reach outside it, how deep it reaches and its limits. The allow
// sheet on an agent and the access key form both use them, and the agent
// tools settings use the depth and limits alone for the defaults.

import { useI18n } from "@maxhq/shared/i18n";

import { type ReactNode, useId } from "react";

import { useView } from "@/app/services";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { FIELD, Label } from "@/settings/controls";
import {
	accountChoices,
	type Choice,
	CONTAINERS,
	LEAVE_CONTAINER,
	type LimitsDraft,
	placeChoices,
	samePlace,
	type ScopeDraft,
	VENDORS,
} from "@maxhq/shared/agent-tools/scope";
import { VENDOR_NAME } from "@maxhq/shared/status/usage";
import { kindLabel, TEST_KINDS } from "@maxhq/shared/test-hosts/hosts";
import { all } from "@maxhq/shared/view/replica";

const toggle = <T,>(list: readonly T[], item: T, on: boolean, same: (a: T, b: T) => boolean) =>
	on ? [...list, item] : list.filter((x) => !same(x, item));

/** A tick box with its name. */
export const Tick = ({
	label,
	checked,
	onChange,
	disabled = false,
}: {
	label: string;
	checked: boolean;
	onChange: (on: boolean) => void;
	disabled?: boolean;
}) => {
	useI18n();
	return (
		<label className="flex min-w-0 items-center gap-2 text-[12.5px] text-foreground">
			<Checkbox
				aria-label={label}
				checked={checked}
				onCheckedChange={onChange}
				disabled={disabled}
			/>
			<span className="truncate">{label}</span>
		</label>
	);
};

/** A named group of tick boxes. */
export const Group = ({ title, children }: { title: string; children: ReactNode }) => {
	useI18n();
	return (
		<div role="group" aria-label={title} className="flex flex-col gap-1.5">
			<Label>{title}</Label>
			<div className="flex flex-wrap gap-x-4 gap-y-1.5">{children}</div>
		</div>
	);
};

// "Any" or a pick from the choices. Unticking any starts with nothing
// picked, and the save says to pick one.
const AnyOr = <T,>({
	title,
	any,
	choices,
	picked,
	same,
	onChange,
}: {
	title: string;
	any: string;
	choices: readonly Choice<T>[];
	picked: readonly T[] | null;
	same: (a: T, b: T) => boolean;
	onChange: (picked: readonly T[] | null) => void;
}) => {
	useI18n();
	return (
		<Group title={title}>
			<Tick
				label={any}
				checked={picked === null}
				onChange={(on) => onChange(on ? null : [])}
			/>
			{picked !== null
				? choices.map((c) => (
						<Tick
							key={c.label}
							label={c.label}
							checked={picked.some((p) => same(p, c.value))}
							onChange={(on) => onChange(toggle(picked, c.value, on, same))}
						/>
					))
				: null}
		</Group>
	);
};

const NumberField = ({
	label,
	value,
	unit,
	onChange,
}: {
	label: string;
	value: string;
	unit?: string;
	onChange: (value: string) => void;
}) => {
	const { t } = useI18n();

	const id = useId();
	return (
		<label htmlFor={id} className="flex min-w-0 flex-col gap-1.5">
			<Label>{label}</Label>
			<span className="flex items-center gap-2 text-xs text-muted-foreground">
				<input
					id={id}
					inputMode="numeric"
					value={value}
					placeholder={t("No limit")}
					onChange={(e) => onChange(e.target.value)}
					spellCheck={false}
					autoComplete="off"
					className={cn(FIELD, "w-[96px] tabular-nums")}
				/>
				{unit}
			</span>
		</label>
	);
};

/** How deep the agents it starts may start their own, and its limits. */
export const LimitFields = ({
	depth,
	limits,
	onChange,
	expiry = true,
}: {
	depth: string;
	limits: LimitsDraft;
	onChange: (change: { depth?: string; limits?: LimitsDraft }) => void;
	/** Whether it offers an expiry, which a default can't have. */
	expiry?: boolean;
}) => {
	const { t } = useI18n();

	const set = (change: Partial<LimitsDraft>) => onChange({ limits: { ...limits, ...change } });
	const id = useId();
	return (
		<div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-3">
			<label htmlFor={id} className="flex min-w-0 flex-col gap-1.5">
				<Label>{t("Depth")}</Label>
				<input
					id={id}
					inputMode="numeric"
					value={depth}
					onChange={(e) => onChange({ depth: e.target.value })}
					spellCheck={false}
					autoComplete="off"
					className={cn(FIELD, "w-[96px] tabular-nums")}
				/>
			</label>
			<NumberField
				label={t("Agents started")}
				value={limits.started}
				onChange={(started) => set({ started })}
			/>
			<NumberField
				label={t("Agents at once")}
				value={limits.running}
				onChange={(running) => set({ running })}
			/>
			<NumberField
				label={t("Usage cap")}
				unit="%"
				value={limits.usage}
				onChange={(usage) => set({ usage })}
			/>
			{expiry ? (
				<NumberField
					label={t("Expires after")}
					unit="days"
					value={limits.days}
					onChange={(days) => set({ days })}
				/>
			) : null}
		</div>
	);
};

/** Every field of a permission's scope. */
export const ScopeFields = ({
	draft,
	onChange,
}: {
	draft: ScopeDraft;
	onChange: (draft: ScopeDraft) => void;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const devices = all(view, "device").sort((a, b) => a.name.localeCompare(b.name));
	const set = (change: Partial<ScopeDraft>) => onChange({ ...draft, ...change });
	return (
		<div className="flex flex-col gap-4">
			<Group title={t("Devices")}>
				{devices.map((d) => (
					<Tick
						key={d.id}
						label={d.name}
						checked={draft.devices.includes(d.id)}
						onChange={(on) =>
							set({ devices: toggle(draft.devices, d.id, on, (a, b) => a === b) })
						}
					/>
				))}
			</Group>
			<Group title={t("Vendors")}>
				{VENDORS.map((v) => (
					<Tick
						key={v}
						label={VENDOR_NAME[v]}
						checked={draft.vendors.includes(v)}
						onChange={(on) =>
							set({ vendors: toggle(draft.vendors, v, on, (a, b) => a === b) })
						}
					/>
				))}
			</Group>
			<AnyOr
				title={t("Accounts")}
				any={t("Any account")}
				choices={accountChoices(view, draft)}
				picked={draft.accounts}
				same={(a, b) => a === b}
				onChange={(accounts) => set({ accounts })}
			/>
			<AnyOr
				title={t("Places")}
				any={t("Anywhere")}
				choices={placeChoices(view, draft)}
				picked={draft.places}
				same={samePlace}
				onChange={(places) => set({ places })}
			/>
			<div className="flex flex-col gap-1">
				<Group title={t("Test hosts")}>
					{TEST_KINDS.map((k) => (
						<Tick
							key={k}
							label={kindLabel(k)}
							checked={draft.testKinds.includes(k)}
							onChange={(on) =>
								set({
									testKinds: toggle(draft.testKinds, k, on, (a, b) => a === b),
								})
							}
						/>
					))}
				</Group>
				<p className="text-[11.5px] text-muted-foreground">
					{t(
						"Leases come from the devices above that host the kind, for agents on them."
					)}
				</p>
			</div>
			<div className="flex flex-col gap-1.5">
				<Label>{t(CONTAINERS)}</Label>
				<div role="group" aria-label={t(CONTAINERS)} className="flex gap-1">
					{LEAVE_CONTAINER.map((c) => (
						<button
							key={c.value}
							type="button"
							aria-pressed={draft.leaveContainer === c.value}
							onClick={() => set({ leaveContainer: c.value })}
							className="h-[26px] rounded-[6px] px-2.5 text-[12.5px] text-secondary-foreground hover:bg-secondary aria-pressed:bg-selected aria-pressed:text-foreground"
						>
							{c.label}
						</button>
					))}
				</div>
				<p className="text-[11.5px] text-muted-foreground">
					{t(
						"Whether an agent in a container may start or reach agents outside it, which run without its network limits and file walls, or use the project's notes, which are shared outside it. For notes, Ask asks once per agent."
					)}
				</p>
			</div>
			<LimitFields depth={draft.depth} limits={draft.limits} onChange={set} />
		</div>
	);
};
