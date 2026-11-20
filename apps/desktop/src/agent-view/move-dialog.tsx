// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Move dialog, from the agent's menu. It picks the device to move the
// agent to, the account there, its own vendor first, the model, and the
// folder there, a checkout of the same repository picked first. A vendor
// not signed in there gets a button that opens its sign-in in Settings.
// It says what carries over and what stays, and while a turn runs its
// button reads Stop and move. The Mac that runs MaxHQ moves the agent, and
// the new one opens once it starts.

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { AGENT_FIELD } from "@/new-agent/agent-pickers";
import { Picker } from "@/new-agent/picker";
import { PathField } from "@/paths/path-field";
import { PRIMARY, SECONDARY } from "@/settings/controls";
import { openSettings } from "@/settings/store";
import { failureOf } from "@maxhq/shared/agent-view/answer";
import {
	agentDevice,
	MOVE_FOLDER,
	type MoveChoices,
	moveForm,
	NO_MOVE_CHOICES,
} from "@maxhq/shared/agent-view/move-agent";
import type { Agent } from "@maxhq/shared/bindings";
import { dispatcherDevice } from "@maxhq/shared/dispatch/reach";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { useNow } from "@maxhq/shared/lib/clock";
import { signInTitle } from "@maxhq/shared/settings/cli-sign-in";
import { deviceName } from "@maxhq/shared/view/select";

import { Spinner } from "./icons";

const Form = ({ agent, onDone }: { agent: Agent; onDone: () => void }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const [choices, setChoices] = useState<MoveChoices>(NO_MOVE_CHOICES);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const form = moveForm(view, agent, choices, now);
	const pick = (patch: Partial<MoveChoices>) => {
		setChoices((c) => ({ ...c, ...patch }));
		setError(null);
	};

	const move = () => {
		const home = dispatcherDevice(view);
		const from = agentDevice(view, agent);
		if (!form.to || home === null || from === null) return;
		setBusy(true);
		setError(null);
		services.source.moveAgent(home, agent.id, from, form.to, form.stop).then(
			(result) => {
				setBusy(false);
				const why = failureOf(result);
				if (why) return setError(why);
				onDone();
				if (result.status === "ok" && result.data.status === "started") {
					openAgent(services, result.data.agent);
				}
			},
			(e: unknown) => {
				setBusy(false);
				setError(
					e instanceof Error && e.message
						? translate("Couldn't {{what}}. {{message}}", {
								what: translate("move the agent"),
								message: e.message,
							})
						: translate("Couldn't {{what}}.", { what: translate("move the agent") })
				);
			}
		);
	};

	const there = form.device === null ? "" : deviceName(view, form.device);
	return (
		<form
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				move();
			}}
		>
			<div className="flex flex-col gap-1">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{translate("Move {{title}} to another device", {
						title: agent.title || translate("This agent"),
					})}
				</Dialog.Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{translate(
						"A new agent takes over on the device you pick, under the same name."
					)}
				</Dialog.Description>
			</div>
			<div className="flex flex-wrap gap-2">
				<Picker
					label={translate("Device")}
					value={form.device ?? ""}
					options={form.devices}
					onChange={(device) => pick({ device, account: null, place: null })}
					disabled={busy}
				/>
				<Picker
					label={translate("Account")}
					value={form.account?.id ?? ""}
					options={form.accounts}
					onChange={(account) => pick({ account, model: null })}
					disabled={busy || form.device === null}
				/>
				<Picker
					label={translate("Model")}
					value={form.model}
					options={form.models}
					onChange={(model) => pick({ model })}
					disabled={busy || !form.account}
				/>
				<Picker
					label={translate("Folder")}
					value={form.place ?? ""}
					options={form.places}
					onChange={(place) => pick({ place })}
					disabled={busy || form.device === null}
					wrapDetails
				/>
			</div>
			{form.place === MOVE_FOLDER ? (
				<PathField
					device={form.device}
					value={choices.path}
					onChange={(path) => pick({ path })}
					label={translate("Folder")}
					title={translate("Choose the agent's folder")}
					placeholder={translate("/Users/you/code/project")}
					disabled={busy}
					className={cn(AGENT_FIELD, "h-8")}
				/>
			) : null}
			{form.missing.length > 0 && form.device !== null ? (
				<div className="flex flex-wrap items-center gap-2">
					<span className="text-[11.5px] text-muted-foreground">
						{translate("Not signed in on {{names}}", { names: there })}
					</span>
					{form.missing.map((vendor) => (
						<button
							key={vendor}
							type="button"
							onClick={() =>
								openSettings(services, {
									kind: "login",
									login: vendor,
									device: form.device ?? undefined,
								})
							}
							className="inline-flex h-6 cursor-pointer items-center rounded-md border border-input px-2 text-[11.5px] font-medium text-secondary-foreground hover:bg-accent"
						>
							{signInTitle(vendor)}
						</button>
					))}
				</div>
			) : null}
			<ul className="flex list-disc flex-col gap-1 pl-4 text-[12.5px] leading-[1.45] text-secondary-foreground">
				{form.lines.map((line) => (
					<li key={line}>{line}</li>
				))}
			</ul>
			{error || form.problem ? (
				<p
					role={error ? "alert" : undefined}
					className={cn(
						"text-[11.5px]",
						error ? "text-status-error-text" : "text-muted-foreground"
					)}
				>
					{error ?? form.problem}
				</p>
			) : null}
			<div className="flex items-center justify-end gap-2">
				<Dialog.Close className={SECONDARY} disabled={busy}>
					{translate("Cancel")}
				</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={busy || form.to === null}>
					{busy ? (
						<Spinner
							size={11}
							className="border-primary-foreground/30 border-t-primary-foreground"
						/>
					) : null}
					{form.action}
				</button>
			</div>
		</form>
	);
};

/** The Move dialog for one agent, open while `open` is. */
export const MoveDialog = ({
	agent,
	open,
	onOpenChange,
}: {
	agent: Agent;
	open: boolean;
	onOpenChange: (open: boolean) => void;
}) => {
	useI18n();
	return (
		<Dialog.Root open={open} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-1/2 left-1/2 z-[121] max-h-[calc(100vh-64px)] w-[min(560px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					<Form agent={agent} onDone={() => onOpenChange(false)} />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
