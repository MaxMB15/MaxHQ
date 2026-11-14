// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sheet that lets an agent run agents through MaxHQ's agent tools: the
// devices, vendors, accounts and places it may start them on, how deep and
// the limits. A new permission starts from the defaults in settings, and
// one the agent holds can change or go.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { ConfirmButton, PRIMARY, SECONDARY } from "@/settings/controls";
import { failureOf } from "@/settings/result";
import type { Agent, AgentDefaults, Permission } from "@maxhq/shared/bindings";
import {
	heldBy,
	offeredOnly,
	permissionFrom,
	type ScopeDraft,
	scopeDraft,
} from "@maxhq/shared/agent-tools/scope";
import { all } from "@maxhq/shared/view/replica";

import { useAgentDefaults, usePermissions, useRefreshAgentTools } from "./queries";
import { ScopeFields } from "./scope-fields";

/**
 * Whether an agent can hold a permission: one MaxHQ runs, which gets the
 * tools over its pipe, and not a sub-agent. One watched in a terminal
 * can't call them as itself.
 */
export const canAllow = (agent: Agent): boolean =>
	agent.control === "held" && agent.parent === null;

/** The permission an agent holds, once the permissions are read. */
export const useHeld = (agent: Agent): Permission | undefined => {
	const permissions = usePermissions().data;
	return permissions?.status === "ok" ? heldBy(permissions.data, agent.id) : undefined;
};

const Form = ({
	agent,
	held,
	defaults,
	onDone,
}: {
	agent: Agent;
	held: Permission | undefined;
	defaults: AgentDefaults;
	onDone: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const refresh = useRefreshAgentTools();
	const home = view.tables.workspace.get(agent.workspace)?.device;
	const [draft, setDraft] = useState<ScopeDraft>(() =>
		scopeDraft(
			held,
			defaults,
			all(view, "device").map((d) => d.id),
			Date.now()
		)
	);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	const act = async (what: string, action: () => Promise<Parameters<typeof failureOf>[0]>) => {
		setBusy(true);
		setError(null);
		try {
			const failure = failureOf(await action());
			await refresh();
			if (failure) setError(failure);
			else onDone();
		} catch (e) {
			console.warn(`Couldn't ${what}`, e);
			setError(t("Couldn't {{what}}. Try again.", { what: what }));
		} finally {
			setBusy(false);
		}
	};

	return (
		<form
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				if (!home) {
					setError(t("Its workspace is gone, so it can't hold a permission."));
					return;
				}
				const parsed = permissionFrom(
					offeredOnly(view, draft),
					{
						id: held?.id ?? crypto.randomUUID(),
						holder: { kind: "agent", agent: agent.id },
						home,
					},
					Date.now()
				);
				if ("error" in parsed) setError(parsed.error);
				else void act(t("save it"), () => source.grantPermission(parsed.ok));
			}}
		>
			<div className="flex flex-col gap-1">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{t("Allow to run agents")}
				</Dialog.Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{t(
						"{{value2}} can start, message and stop agents within these bounds. Depth counts the levels of agents below it.",
						{ value2: agent.title || t("This agent") }
					)}
				</Dialog.Description>
			</div>
			<ScopeFields
				draft={draft}
				onChange={(next) => {
					setDraft(next);
					setError(null);
				}}
			/>
			{error ? (
				<p role="alert" className="text-[11.5px] text-status-error-text">
					{error}
				</p>
			) : null}
			<div className="flex items-center gap-2">
				{held ? (
					<span className="mr-auto">
						<ConfirmButton
							label={t("Revoke")}
							question={t("It can't start agents after this.")}
							confirm={t("Revoke")}
							disabled={busy}
							onConfirm={() =>
								void act(t("revoke it"), () => source.revokePermission(held.id))
							}
						/>
					</span>
				) : null}
				<Dialog.Close className={SECONDARY} disabled={busy}>
					{t("Cancel")}
				</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={busy}>
					{held ? t("Save") : t("Allow")}
				</button>
			</div>
		</form>
	);
};

/** The allow sheet for one agent, open while `open` is. */
export const AllowSheet = ({
	agent,
	open,
	onOpenChange,
}: {
	agent: Agent;
	open: boolean;
	onOpenChange: (open: boolean) => void;
}) => {
	const { t } = useI18n();

	const permissions = usePermissions();
	const defaults = useAgentDefaults();
	const ready = permissions.data?.status === "ok" && defaults.data;
	const failed = permissions.error || defaults.error || permissions.data?.status === "error";
	const held = useHeld(agent);
	return (
		<Dialog.Root open={open} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup
					aria-label={t("Allow to run agents")}
					className="fixed top-1/2 left-1/2 z-[121] max-h-[calc(100vh-64px)] w-[min(560px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border border-input bg-card p-5 shadow-popup outline-none"
				>
					{ready && defaults.data ? (
						<Form
							agent={agent}
							held={held}
							defaults={defaults.data}
							onDone={() => onOpenChange(false)}
						/>
					) : failed ? (
						<p role="alert" className="text-[12.5px] text-status-error-text">
							{t("Couldn't read the permissions. Try again.")}
						</p>
					) : (
						<p className="text-[12.5px] text-muted-foreground">
							{t("Reading permissions")}
						</p>
					)}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
