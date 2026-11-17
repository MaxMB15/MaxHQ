// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent tools page: the depth and limits a new permission starts with,
// and the access keys that let tools outside MaxHQ run agents. A key's
// file stays on its device, so the page shows where it is and the command
// that uses it, never the key.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import {
	useAgentDefaults,
	useKeys,
	usePermissions,
	useRefreshAgentTools,
} from "@/agent-tools/queries";
import { LimitFields, ScopeFields } from "@/agent-tools/scope-fields";
import { CopyButton } from "@/components/copy-button";
import { CopyIcon } from "@/components/copy-field";
import { Picker } from "@/new-agent/picker";
import { type AccessKey, type AgentDefaults, DEPTH, type Permission } from "@maxhq/shared/bindings";
import { keyGrants } from "@maxhq/shared/agent-tools/grants";
import {
	commandLine,
	depthFrom,
	keyNameError,
	limitsDraft,
	limitsFrom,
	limitsLine,
	offeredOnly,
	permissionFrom,
	scopeDraft,
	type ScopeDraft,
} from "@maxhq/shared/agent-tools/scope";
import { useNow } from "@maxhq/shared/lib/clock";
import { all } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";

import {
	ConfirmButton,
	FIELD,
	Label,
	PRIMARY,
	Row,
	SECONDARY,
	Section,
	StatusNote,
} from "./controls";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const DefaultsForm = ({ saved }: { saved: AgentDefaults }) => {
	useI18n();
	const { source } = useServices();
	const refresh = useRefreshAgentTools();
	const [depth, setDepth] = useState(String(saved.depth));
	const [limits, setLimits] = useState(() => limitsDraft(saved.limits, Date.now()));
	const [note, setNote] = useState<Note | null>(null);

	const save = async () => {
		const parsedDepth = depthFrom(depth);
		if ("error" in parsedDepth) return setNote({ text: parsedDepth.error, error: true });
		const parsed = limitsFrom(limits, Date.now());
		if ("error" in parsed) return setNote({ text: parsed.error, error: true });
		setNote({ text: translate("Saving"), error: false });
		try {
			// A default has no expiry, since it would date from the save.
			const next = { depth: parsedDepth.ok, limits: { ...parsed.ok, expires_at: null } };
			const failure = failureOf(await source.setAgentDefaults(next));
			await refresh();
			setNote(
				failure
					? { text: failure, error: true }
					: { text: translate("Saved"), error: false }
			);
		} catch (error) {
			console.warn(translate("Couldn't save the agent tools defaults"), error);
			setNote({ text: translate("Couldn't save it. Try again."), error: true });
		}
	};

	return (
		<div className="flex flex-col gap-3 px-4 py-3">
			<LimitFields
				depth={depth}
				limits={limits}
				onChange={(change) => {
					setNote(null);
					if (change.depth !== undefined) setDepth(change.depth);
					if (change.limits) setLimits(change.limits);
				}}
				expiry={false}
			/>
			<p className="text-xs text-muted-foreground">
				{translate(
					"Depth counts the levels of agents below the one that holds the permission. Empty limits are off. Limits count agents and usage, they don't fence off files. An agent that should stay away from your files runs in a container, with its permission's Containers setting on Ask or Never."
				)}
			</p>
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} />
				<button type="button" className={PRIMARY} onClick={() => void save()}>
					{translate("Save defaults")}
				</button>
			</div>
		</div>
	);
};

const KeyRow = ({ accessKey, permission }: { accessKey: AccessKey; permission?: Permission }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const refresh = useRefreshAgentTools();
	const [note, setNote] = useState<Note | null>(null);
	const revoke = async () => {
		try {
			const failure = failureOf(await source.revokePermission(accessKey.permission));
			if (failure) setNote({ text: failure, error: true });
			await refresh();
		} catch (error) {
			console.warn(translate("Couldn't revoke the key"), error);
			setNote({ text: translate("Couldn't revoke it. Try again."), error: true });
		}
	};
	return (
		<Row
			title={accessKey.name}
			sub={
				<>
					<span className="block">
						{translate("On ")}
						{deviceName(view, accessKey.device)}.{" "}
						{permission ? limitsLine(permission.limits, now) : null}
					</span>
					<span className="flex min-w-0 items-center gap-0.5">
						<span title={accessKey.path} className="min-w-0 truncate font-mono">
							{accessKey.path}
						</span>
						<CopyIcon
							text={accessKey.path}
							label={translate("Copy path")}
							className="size-[18px]"
						/>
					</span>
				</>
			}
		>
			<StatusNote note={note} />
			<CopyButton
				text={commandLine(accessKey.command)}
				label={translate("Copy MCP command")}
				className={SECONDARY}
			/>
			<ConfirmButton
				label={translate("Revoke")}
				question="Tools using it stop working."
				confirm={translate("Revoke")}
				onConfirm={() => void revoke()}
			/>
		</Row>
	);
};

const MintForm = ({ defaults }: { defaults: AgentDefaults }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const refresh = useRefreshAgentTools();
	const devices = all(view, "device").sort((a, b) => a.name.localeCompare(b.name));
	const [device, setDevice] = useState(devices[0]?.id ?? "");
	const [name, setName] = useState("");
	const [draft, setDraft] = useState<ScopeDraft>(() =>
		scopeDraft(
			undefined,
			defaults,
			devices.map((d) => d.id),
			Date.now()
		)
	);
	const [note, setNote] = useState<Note | null>(null);
	const [busy, setBusy] = useState(false);

	const mint = async () => {
		const nameError = keyNameError(name);
		if (nameError) return setNote({ text: nameError, error: true });
		if (!device)
			return setNote({ text: translate("Pick the device that keeps the key."), error: true });
		const id = crypto.randomUUID();
		const parsed = permissionFrom(
			offeredOnly(view, draft),
			{ id, holder: { kind: "key", key: id }, home: device },
			Date.now()
		);
		if ("error" in parsed) return setNote({ text: parsed.error, error: true });
		setBusy(true);
		setNote({ text: translate("Making the key"), error: false });
		try {
			const result = await source.mintKey(device, name.trim(), parsed.ok);
			// A key can be made even when a device missed its permission, so
			// the list is read again either way.
			await refresh();
			const failure = failureOf(result);
			if (failure) {
				setNote({ text: failure, error: true });
			} else if (result.status === "ok" && result.data?.status === "key_minted") {
				setNote({
					text: translate("Made {{value1}}. Its file is {{path}}", {
						value1: name.trim(),
						path: result.data.path,
					}),
					error: false,
				});
				setName("");
			}
		} catch (error) {
			console.warn(translate("Couldn't make the key"), error);
			setNote({ text: translate("Couldn't make the key. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	return (
		<form
			aria-label={translate("New access key")}
			className="flex flex-col gap-4 border-t border-sidebar-border px-4 py-3"
			onSubmit={(e) => {
				e.preventDefault();
				void mint();
			}}
		>
			<div className="flex flex-wrap items-end gap-3">
				<label className="flex min-w-[180px] flex-1 flex-col gap-1.5">
					<Label>{translate("Name")}</Label>
					<input
						aria-label={translate("Key name")}
						value={name}
						placeholder={translate("slack-bot")}
						onChange={(e) => {
							setName(e.target.value);
							setNote(null);
						}}
						spellCheck={false}
						autoComplete="off"
						className={FIELD}
					/>
				</label>
				<Picker
					label={translate("Kept on")}
					value={device}
					options={devices.map((d) => ({ value: d.id, label: d.name }))}
					onChange={setDevice}
				/>
			</div>
			<ScopeFields
				draft={draft}
				onChange={(next) => {
					setDraft(next);
					setNote(null);
				}}
			/>
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="min-w-0 break-all" />
				<button type="submit" className={PRIMARY} disabled={busy}>
					{translate("Make key")}
				</button>
			</div>
		</form>
	);
};

/** The page's headings and labels, for the settings search. */
export const AGENT_TOOLS_TERMS: readonly string[] = [
	"New permissions",
	"Depth",
	"Agents started",
	"Agents at once",
	"Usage cap",
	"Expires after",
	"Devices",
	"Vendors",
	"Accounts",
	"Places",
	"Test hosts",
	"Access keys",
	"For tools outside MaxHQ, such as a bot",
	"New access key",
	"Key name",
	"Copy MCP command",
	"Revoke",
];

/** The Agent tools page. */
export const AgentToolsSettingsPage = () => {
	useI18n();
	const defaults = useAgentDefaults();
	const keys = useKeys();
	const permissions = usePermissions();
	const held = permissions.data?.status === "ok" ? permissions.data.data : [];
	const listed = keys.data?.status === "ok" ? keys.data.data : null;
	return (
		<>
			<Section
				title={translate("New permissions")}
				hint={translate("{{DEPTH}} levels deep and no limits by default", { DEPTH: DEPTH })}
			>
				{defaults.data ? (
					<DefaultsForm saved={defaults.data} />
				) : (
					<Row
						title={
							defaults.error
								? translate("Couldn't read the defaults")
								: translate("Reading the defaults")
						}
					/>
				)}
			</Section>
			<Section
				title={translate("Access keys")}
				hint={translate("For tools outside MaxHQ, such as a bot")}
			>
				{listed === null ? (
					<Row
						title={
							keys.data?.status === "error" || keys.error
								? translate("Couldn't read the keys")
								: translate("Reading the keys")
						}
					/>
				) : listed.length === 0 ? (
					<Row title={translate("No keys yet")} />
				) : (
					keyGrants(listed, held).map(({ key, permission }) => (
						<KeyRow key={key.id} accessKey={key} permission={permission} />
					))
				)}
				{defaults.data ? <MintForm defaults={defaults.data} /> : null}
			</Section>
		</>
	);
};
