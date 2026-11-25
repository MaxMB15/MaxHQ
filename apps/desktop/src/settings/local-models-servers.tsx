// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Servers card of the Local models page: the llama.cpp server MaxHQ
// runs on a device, with its install and the model it has loaded, and the
// server URLs the device reaches, each with what answers there and its
// last test. The managed server's port and key stay with `maxd`. A form
// adds a server URL: Test asks what answers there and shows it, saving
// nothing, and Add saves it. Its token goes from a password field straight
// into the test or the add, and Add clears it; no state, log or note keeps
// it, and a reply that echoes it shows `<secret>`. Plain http to another
// machine is still allowed, with the risk named on the row and in the form.
// The form's choice at the top picks your own server or OpenRouter, whose
// URL is set and which signs in or takes a pasted key; its parts are in
// local-models-openrouter. Under the managed server, a switch says whether
// linked devices' Local agents may use its models over their maxd link.

import { Fragment, type ReactNode, useRef, useState } from "react";

import { useServices } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { Badge } from "@/tasks/activity";
import type { DeviceId, ModelServer } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import {
	canSignInWithOpenRouter,
	OPENROUTER_LABEL,
	OPENROUTER_URL,
} from "@maxhq/shared/local-models/openrouter";
import { serversOf } from "@maxhq/shared/local-models/select";
import {
	addedServerOf,
	addServerAction,
	canAddServer,
	managedServerView,
	notEncryptedDetail,
	probeFindings,
	type ProbeFindings,
	probeServerAction,
	probeSummary,
	removeServerQuestion,
	serverUrlProblem,
	typedNotEncrypted,
} from "@maxhq/shared/local-models/settings-page";
import {
	flavorName,
	linkedDevicesHelp,
	linkedDevicesLabel,
	notEncrypted,
	notEncryptedText,
} from "@maxhq/shared/local-models/wording";
import type { ViewState } from "@maxhq/shared/view/replica";

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
import { useModelAction } from "./local-models-catalog";
import {
	OpenRouterSignIn,
	ServerChoice,
	type ServerKind,
	useSignInAgain,
} from "./local-models-openrouter";
import { SecretField } from "./login-parts";
import { failureOf, type Note, scrub } from "./result";

/** A server's last test, in red when it failed. */
const ProbeLine = ({ server }: { server: Pick<ModelServer, "probe"> }) => {
	useI18n();
	const probe = probeSummary(server);
	return (
		<span
			className={cn(probe.ok === false ? "text-status-error-text" : "text-muted-foreground")}
		>
			{probe.text}
		</span>
	);
};

/** The note on a server whose traffic crosses the network unencrypted, with the risk as its tooltip. */
const NotEncryptedBadge = () => {
	useI18n();
	return (
		<span title={notEncryptedDetail()} className="inline-flex">
			<Badge tone="warn">{notEncryptedText()}</Badge>
		</span>
	);
};

/** The server MaxHQ runs on the device: its install, its loaded model and who else uses it. */
const ManagedRow = ({
	device,
	view,
	server,
	disabled,
}: {
	device: DeviceId;
	view: ViewState;
	server: ModelServer;
	disabled: string | null;
}) => {
	useI18n();
	const { busy, note, send } = useModelAction(device);
	const shown = managedServerView(view, server);
	return (
		<div
			role="group"
			aria-label={translate("Run by MaxHQ")}
			className="flex items-start gap-4 border-t border-sidebar-border px-4 py-3 first:border-t-0"
		>
			<div className="flex min-w-0 flex-1 flex-col gap-1 text-xs">
				<span className="text-[12.5px] text-foreground">{translate("Run by MaxHQ")}</span>
				<span
					className={cn(shown.error ? "text-status-error-text" : "text-muted-foreground")}
				>
					{shown.state}
				</span>
				{shown.model ? (
					<span className="flex items-center gap-2 text-muted-foreground">
						<Badge tone={shown.model.tone}>{shown.model.load}</Badge>
						{shown.model.name}
					</span>
				) : (
					<span className="text-muted-foreground">{translate("No model loaded")}</span>
				)}
				<ProbeLine server={server} />
				{shown.users ? <span className="text-muted-foreground">{shown.users}</span> : null}
			</div>
			<div className="flex shrink-0 items-center gap-2.5">
				<StatusNote note={note} />
				<button
					type="button"
					className={SECONDARY}
					disabled={disabled !== null || busy}
					onClick={() => void send({ action: "test_server", server: server.id })}
				>
					{translate("Test")}
				</button>
			</div>
		</div>
	);
};

/** A server URL the device reaches, with what answers there, its last test, Test and Remove. */
const ExternalRow = ({
	device,
	server,
	disabled,
}: {
	device: DeviceId;
	server: ModelServer;
	disabled: string | null;
}) => {
	useI18n();
	const { busy, note, send } = useModelAction(device);
	const flavor = server.kind.type === "external" ? server.kind.flavor : null;
	const off = disabled !== null || busy;
	const again = useSignInAgain(device, server, off);
	return (
		<div
			role="group"
			aria-label={server.label}
			className="flex items-start gap-4 border-t border-sidebar-border px-4 py-3 first:border-t-0"
		>
			<div className="flex min-w-0 flex-1 flex-col gap-1 text-xs">
				<span className="flex items-center gap-2">
					<span className="truncate text-[12.5px] text-foreground">{server.label}</span>
					{flavor && server.probe && !server.probe.error ? (
						<span className="text-muted-foreground">{flavorName(flavor)}</span>
					) : null}
					{notEncrypted(server) ? <NotEncryptedBadge /> : null}
				</span>
				{server.base_url ? (
					<span className="selectable truncate font-mono text-muted-foreground">
						{server.base_url}
					</span>
				) : null}
				<ProbeLine server={server} />
				{server.token ? (
					<span className="text-muted-foreground">{translate("Sends a token")}</span>
				) : null}
				{again.line}
			</div>
			<div className="flex shrink-0 items-center gap-2.5">
				<StatusNote note={note} />
				{again.button}
				<button
					type="button"
					className={SECONDARY}
					disabled={off}
					onClick={() => void send({ action: "test_server", server: server.id })}
				>
					{translate("Test")}
				</button>
				<ConfirmButton
					label={translate("Remove")}
					question={removeServerQuestion()}
					confirm={translate("Remove")}
					onConfirm={() => void send({ action: "remove_server", server: server.id })}
					disabled={off}
				/>
			</div>
		</div>
	);
};

/** A server the form knows, such as OpenRouter's: its name and URL are set, and it asks only for a key. */
type ServerPreset = {
	readonly label: string;
	readonly url: string;
	/** The key field's name. */
	readonly keyLabel: string;
	/** Whether a sign-in above is the form's main button, so Add isn't. */
	readonly signIn: boolean;
};

/**
 * The fields and buttons that add a server URL. Test asks the device what
 * answers at the URL and shows it, keeping nothing; Add saves the server,
 * and Cancel closes the form. Changing the URL or the token clears what the
 * last Test found. With a preset, they ask only for the preset's key, which
 * Test and Add need.
 */
const AddServerForm = ({
	device,
	onClose,
	top,
	preset,
}: {
	device: DeviceId;
	onClose: () => void;
	/** What goes above the fields, such as OpenRouter's sign-in. */
	top?: ReactNode;
	preset?: ServerPreset;
}) => {
	useI18n();
	const services = useServices();
	const { source } = services;
	const token = useRef<HTMLInputElement>(null);
	const [label, setLabel] = useState(preset?.label ?? "");
	const [url, setUrl] = useState(preset?.url ?? "");
	const [typed, setTyped] = useState(false);
	const [found, setFound] = useState<ProbeFindings | null>(null);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const problem = serverUrlProblem(url);
	const ready = canAddServer(url) && !busy && (!preset || typed);

	/** Sends one action, and says why it failed. */
	const run = async (
		send: () => Promise<ActionResult>,
		secret: string
	): Promise<ActionResult | null> => {
		setBusy(true);
		setNote(null);
		try {
			const result = await send();
			const failure = failureOf(result);
			if (failure) {
				setNote({ text: scrub(failure, secret), error: true });
				return null;
			}
			return result;
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			console.warn(translate("Couldn't change the servers:"), scrub(reason, secret));
			setNote({ text: translate("Couldn't do it. Try again."), error: true });
			return null;
		} finally {
			setBusy(false);
		}
	};

	/** Tests the URL with the token typed, which stays in its field for Add. */
	const test = async () => {
		const secret = token.current?.value.trim() ?? "";
		setFound(null);
		const result = await run(
			() => source.modelAction(device, probeServerAction(url, secret)),
			secret
		);
		if (result) setFound(probeFindings(services.view.getState().view, device, result));
	};

	/** Saves the server with the token typed, which it reads and clears. */
	const add = async () => {
		const input = token.current;
		const secret = input?.value.trim() ?? "";
		if (input) input.value = "";
		setTyped(false);
		const result = await run(
			() => source.modelAction(device, addServerAction(label, url, secret)),
			secret
		);
		if (result && addedServerOf(result)) onClose();
	};

	return (
		<>
			{top}
			{preset ? null : (
				<div className="grid grid-cols-2 gap-3">
					<label className="flex flex-col gap-1.5">
						<Label>{translate("Name")}</Label>
						<input
							aria-label={translate("Name")}
							value={label}
							placeholder={translate("Ollama on devbox")}
							onChange={(e) => setLabel(e.target.value)}
							spellCheck={false}
							autoComplete="off"
							className={FIELD}
						/>
					</label>
					<label className="flex flex-col gap-1.5">
						<Label>{translate("URL")}</Label>
						<input
							aria-label={translate("URL")}
							value={url}
							placeholder="http://devbox:11434"
							onChange={(e) => {
								setUrl(e.target.value);
								setFound(null);
							}}
							spellCheck={false}
							autoComplete="off"
							autoCapitalize="off"
							autoCorrect="off"
							className={cn(FIELD, "font-mono text-xs")}
						/>
					</label>
				</div>
			)}
			<SecretField
				field={token}
				onTyped={(any) => {
					setTyped(any);
					setFound(null);
				}}
				label={preset?.keyLabel ?? translate("Token, if the server asks for one")}
				placeholder={translate("Kept by maxd, never shown again")}
			/>
			{problem ? <p className="text-xs text-status-error-text">{problem}</p> : null}
			{typedNotEncrypted(url) ? (
				<p className="flex flex-col gap-1 text-xs text-muted-foreground">
					<span className="font-[600] text-status-busy-text">{notEncryptedText()}</span>
					{notEncryptedDetail()}
				</p>
			) : null}
			{busy && !found ? (
				<p className="text-xs text-muted-foreground">{translate("Testing")}</p>
			) : found ? (
				<div
					role="status"
					aria-label={translate("What the test found")}
					className="flex flex-col gap-1 text-xs"
				>
					<span className="flex items-center gap-2">
						{found.flavor ? (
							<span className="font-[600] text-foreground">{found.flavor}</span>
						) : null}
						<span
							className={cn(
								found.summary.ok === false
									? "text-status-error-text"
									: "text-muted-foreground"
							)}
						>
							{found.summary.text}
						</span>
					</span>
					{found.models ? (
						<span className="text-muted-foreground">{found.models}</span>
					) : null}
				</div>
			) : null}
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="mr-auto" />
				<button type="button" className={SECONDARY} disabled={busy} onClick={onClose}>
					{translate("Cancel")}
				</button>
				<button
					type="button"
					className={SECONDARY}
					disabled={!ready}
					onClick={() => void test()}
				>
					{translate("Test")}
				</button>
				<button
					type="button"
					className={preset?.signIn ? SECONDARY : PRIMARY}
					disabled={!ready}
					onClick={() => void add()}
				>
					{translate("Add")}
				</button>
			</div>
		</>
	);
};

/**
 * The form that adds a server, with the choice at its top: your own server
 * at a URL, or OpenRouter, which signs in or takes a pasted key. Each
 * choice starts with its fields empty.
 */
const AddServer = ({
	device,
	view,
	onClose,
}: {
	device: DeviceId;
	view: ViewState;
	onClose: () => void;
}) => {
	useI18n();
	const [kind, setKind] = useState<ServerKind>("own");
	return (
		<div
			role="group"
			aria-label={translate("Add server URL")}
			className="flex flex-col gap-3 border-t border-sidebar-border px-4 py-3"
		>
			<ServerChoice value={kind} onChange={setKind} />
			{kind === "own" ? (
				<AddServerForm key={kind} device={device} onClose={onClose} />
			) : (
				<AddServerForm
					key={kind}
					device={device}
					onClose={onClose}
					preset={{
						label: OPENROUTER_LABEL,
						url: OPENROUTER_URL,
						keyLabel: translate("API key"),
						signIn: canSignInWithOpenRouter(view, device),
					}}
					top={<OpenRouterSignIn device={device} view={view} onSignedIn={onClose} />}
				/>
			)}
		</div>
	);
};

/** The switch that lets linked devices use the managed server's models, as the page saves it. */
export type SharingSwitch = {
	readonly on: boolean;
	readonly note: Note | null;
	readonly onChange: (on: boolean) => void;
};

/** Whether linked devices' Local agents may use the managed server's models. */
const SharingRow = ({ sharing, disabled }: { sharing: SharingSwitch; disabled: string | null }) => {
	useI18n();
	return (
		<Row title={linkedDevicesLabel()} sub={disabled ?? linkedDevicesHelp()}>
			<StatusNote note={sharing.note} />
			<Switch
				aria-label={linkedDevicesLabel()}
				checked={sharing.on}
				disabled={disabled !== null}
				onCheckedChange={sharing.onChange}
			/>
		</Row>
	);
};

/**
 * The Servers card: the server MaxHQ runs on the device with the switch
 * that shares it, the server URLs it reaches, and the form that adds one.
 */
export const ServersSection = ({
	device,
	view,
	disabled,
	adding,
	onAdding,
	sharing,
}: {
	device: DeviceId;
	view: ViewState;
	/** Why nothing can change now, or null when it can. */
	disabled: string | null;
	/** Whether the form that adds a server URL is open. */
	adding: boolean;
	onAdding: (adding: boolean) => void;
	/** The switch under the managed server, on a device that runs one. */
	sharing: SharingSwitch | null;
}) => {
	useI18n();
	const servers = serversOf(view, device);
	return (
		<Section title={translate("Servers")} hint={translate("Local agents run on these")}>
			{servers.map((server) =>
				server.kind.type === "managed" ? (
					<Fragment key={server.id}>
						<ManagedRow
							device={device}
							view={view}
							server={server}
							disabled={disabled}
						/>
						{sharing ? <SharingRow sharing={sharing} disabled={disabled} /> : null}
					</Fragment>
				) : (
					<ExternalRow
						key={server.id}
						device={device}
						server={server}
						disabled={disabled}
					/>
				)
			)}
			{servers.length === 0 ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{translate("No servers yet")}
				</p>
			) : null}
			{adding && disabled === null ? (
				<AddServer device={device} view={view} onClose={() => onAdding(false)} />
			) : (
				<div className="flex items-center gap-2.5 border-t border-sidebar-border px-4 py-3">
					<button
						type="button"
						className={SECONDARY}
						disabled={disabled !== null}
						onClick={() => onAdding(true)}
					>
						{translate("Add server URL")}
					</button>
					{disabled ? (
						<span className="text-xs text-muted-foreground">{disabled}</span>
					) : null}
				</div>
			)}
		</Section>
	);
};
