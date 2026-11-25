// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Add device wizard, one step at a time: pick a host from your SSH
// config, your tailnet or a Windows device's WSL distros, choose the logins
// it gets, then follow the app as
// it reads the host key, waits for you to trust it, installs maxd, adds
// MaxHQ's key, connects through it and sends the logins. Nothing runs on
// the host before you trust its key, and a host that can't keep maxd
// running shows the command to fix it.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { type FormEvent, useEffect, useState } from "react";

import { useServices, useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { CopyIcon } from "@/components/copy-field";
import { ConnectionDot } from "@/components/status/dots";
import { cn } from "@/lib/utils";
import { Picker } from "@/new-agent/picker";
import { FIELD, Label, PRIMARY, SECONDARY } from "@/settings/controls";
import { scrub } from "@/settings/result";
import type { AccountId, DeviceId, LinkId } from "@maxhq/shared/bindings";
import type {
	SetupHint,
	SetupHost,
	SetupPhase,
	SetupProgress,
	SetupSecret,
	SetupStep,
	SetupWsl,
} from "@maxhq/shared/fleet/wire";
import type { ConnectionLook } from "@maxhq/shared/status/connection";
import { isLocalAccount } from "@maxhq/shared/status/agent-kind";
import { accountName } from "@maxhq/shared/status/usage";

/** The steps after the pick, which the list counts down. */
const STEPS: readonly Exclude<SetupStep, "pick">[] = [
	"host_key",
	"install",
	"key",
	"handshake",
	"secrets",
];

/** What the list says of a step's phase. The app sends the phase, not the words. */
export const phaseText = (phase: SetupPhase, host: string): string => {
	switch (phase) {
		case "reading_host_key":
			return t("Reading {{host}}'s host key", { host: host });
		case "checking_system":
			return t("Checking the system");
		case "copying_maxd":
			return t("Copying maxd");
		case "added_key":
			return t("Added MaxHQ's key");
		case "connecting":
			return t("Connecting to maxd");
		case "waiting_for_answer":
			return t("Waiting for your answer about which device this is");
		case "sending_logins":
			return t("Sending the logins you picked");
	}
};

/** What a hint asks you to do on the host. */
const hintText = (kind: SetupHint): string => {
	switch (kind) {
		case "linger":
			return t("maxd can't keep running after you log out until lingering is on.");
	}
};

const Title = ({ children }: { children: string }) => {
	useI18n();
	return (
		<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
			{children}
		</Dialog.Title>
	);
};

const Description = ({ children }: { children: string }) => {
	useI18n();
	return (
		<Dialog.Description className="text-[12.5px] text-muted-foreground">
			{children}
		</Dialog.Description>
	);
};

const fromText = (host: SetupHost) => {
	if (host.wsl) return wslText(host.wsl);
	const from = host.from === "ssh_config" ? t("From your SSH config") : t("On your tailnet");
	return host.address ? `${from}, ${host.address}` : from;
};

/** Where a WSL distro is, and whether it's the default one. */
const wslText = (wsl: SetupWsl) => {
	const on = wsl.here ? t("WSL on this PC") : t("WSL on {{machine}}", { machine: wsl.machine });
	return wsl.default ? `${on}, ${t("the default distro")}` : on;
};

/**
 * Why a host can't be set up, or null when it can. A WSL 1 distro runs no
 * systemd, so maxd wouldn't keep running there.
 */
const unavailable = (host: SetupHost): string | null =>
	host.wsl?.version === 1
		? t("WSL 1 can't run maxd. Move it to WSL 2 with wsl --set-version {{distro}} 2.", {
				distro: host.wsl.distro,
			})
		: null;

/** What the wizard calls a host: a WSL distro by its name. */
export const hostName = (hosts: readonly SetupHost[] | null, host: string): string =>
	hosts?.find((h) => h.host === host)?.wsl?.distro ?? host;

/**
 * The machine whose host key setup reads for `host`: a WSL distro's
 * Windows machine, which it reaches over SSH.
 */
const keyHost = (hosts: readonly SetupHost[] | null, host: string): string => {
	const wsl = hosts?.find((h) => h.host === host)?.wsl;
	return wsl && !wsl.here ? wsl.machine : hostName(hosts, host);
};

/** Step 1: the host. */
const Pick = ({
	hosts,
	host,
	onHost,
	onNext,
}: {
	hosts: readonly SetupHost[] | null;
	host: string;
	onHost: (host: string) => void;
	onNext: () => void;
}) => {
	const { t } = useI18n();

	const listed = hosts?.some((h) => h.host === host) ?? false;
	const submit = (event: FormEvent) => {
		event.preventDefault();
		if (host.trim() !== "") onNext();
	};
	return (
		<form onSubmit={submit} className="flex flex-col gap-4">
			<div className="flex flex-col gap-1">
				<Title>{t("Add a device")}</Title>
				<Description>
					{t("Pick the machine to set up. MaxHQ signs in with your own SSH login.")}
				</Description>
			</div>
			<div role="radiogroup" aria-label={t("Hosts")} className="flex flex-col gap-1.5">
				{hosts === null ? (
					<p className="text-xs text-muted-foreground">{t("Looking for hosts")}</p>
				) : hosts.length === 0 ? (
					<p className="text-xs text-muted-foreground">
						{t("No hosts in your SSH config or on your tailnet. Type one below.")}
					</p>
				) : (
					hosts.map((h) => (
						<button
							key={h.host}
							type="button"
							role="radio"
							aria-checked={host === h.host}
							disabled={h.link !== null || unavailable(h) !== null}
							onClick={() => onHost(h.host)}
							className={cn(
								"flex items-center gap-3 rounded-lg border px-3 py-2 text-left disabled:opacity-60",
								host === h.host
									? "border-primary bg-primary/10"
									: "border-sidebar-border bg-card enabled:hover:bg-secondary"
							)}
						>
							<span className="min-w-0 flex-1">
								<span className="block truncate text-[12.5px] text-foreground">
									{h.wsl?.distro ?? h.host}
								</span>
								<span className="block truncate text-xs text-muted-foreground">
									{fromText(h)}
								</span>
								{unavailable(h) !== null && h.link === null ? (
									<span className="block text-xs text-muted-foreground">
										{unavailable(h)}
									</span>
								) : null}
							</span>
							{h.link !== null ? (
								<span className="shrink-0 text-xs text-muted-foreground">
									{t("Set up already")}
								</span>
							) : null}
						</button>
					))
				)}
			</div>
			<label className="flex flex-col gap-1.5">
				<Label>{t("Or another host")}</Label>
				<input
					aria-label={t("Another host")}
					placeholder={t("user@host, as ssh takes it")}
					value={listed ? "" : host}
					onChange={(event) => onHost(event.target.value.trim())}
					className={FIELD}
				/>
			</label>
			<div className="flex justify-end gap-2">
				<Dialog.Close className={SECONDARY}>{t("Cancel")}</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={host === ""}>
					{t("Next")}
				</button>
			</div>
		</form>
	);
};

/** Step 2: the logins the device gets once it's linked. */
const Logins = ({
	host,
	accounts,
	account,
	token,
	onAccount,
	onToken,
	onBack,
	onStart,
}: {
	host: string;
	accounts: readonly { id: AccountId; label: string }[];
	account: AccountId | null;
	token: string;
	onAccount: (account: AccountId) => void;
	onToken: (token: string) => void;
	onBack: () => void;
	onStart: () => void;
}) => {
	const { t } = useI18n();
	return (
		<div className="flex flex-col gap-4">
			<div className="flex flex-col gap-1">
				<Title>{t("Logins for {{host}}", { host: host })}</Title>
				<Description>
					{t(
						"For personal use, paste a Claude token from claude setup-token. This turns setup-tokens on for the device: it stores your token and gives it to Claude Code agents when they start there. MaxHQ keeps no copy. Leave it empty to sign in with claude on the device instead."
					)}
				</Description>
			</div>
			{accounts.length > 1 ? (
				<Picker
					label={t("Claude account")}
					value={account ?? ""}
					options={accounts.map((a) => ({ value: a.id, label: a.label }))}
					onChange={onAccount}
				/>
			) : null}
			<label className="flex flex-col gap-1.5">
				<Label>{t("Claude token")}</Label>
				<input
					aria-label={t("Claude token")}
					type="password"
					autoComplete="off"
					value={token}
					onChange={(event) => onToken(event.target.value)}
					disabled={accounts.length === 0}
					className={FIELD}
				/>
			</label>
			<div className="flex justify-end gap-2">
				<button type="button" className={SECONDARY} onClick={onBack}>
					{t("Back")}
				</button>
				<button type="button" className={PRIMARY} onClick={onStart}>
					{t("Set up {{host}}", { host: host })}
				</button>
			</div>
		</div>
	);
};

type HostKey = Extract<SetupProgress, { type: "host_key" }>;
type Hint = Extract<SetupProgress, { type: "hint" }>;

/** Where a setup stands, from what the app has sent. */
export type RunState = {
	readonly setup: string | null;
	/** The steps reached so far, in order, each at the phase it's reached. */
	readonly steps: readonly { step: SetupStep; phase: SetupPhase }[];
	readonly hostKey: HostKey | null;
	readonly hint: Hint | null;
	readonly failed: { step: SetupStep; reason: string } | null;
	readonly done: { link: LinkId; device: DeviceId } | null;
};

export const START: RunState = {
	setup: null,
	steps: [],
	hostKey: null,
	hint: null,
	failed: null,
	done: null,
};

/** The state after one message from the app. */
export const follow = (state: RunState, progress: SetupProgress): RunState => {
	const at = { ...state, setup: progress.setup };
	switch (progress.type) {
		case "step":
			return {
				...at,
				steps: [
					...state.steps.filter((s) => s.step !== progress.step),
					{ step: progress.step, phase: progress.phase },
				],
			};
		case "host_key":
			return { ...at, hostKey: progress };
		case "hint":
			return { ...at, hint: progress };
		case "failed":
			return {
				...at,
				hostKey: null,
				failed: { step: progress.step, reason: progress.reason },
			};
		case "done":
			return { ...at, hostKey: null, done: { link: progress.link, device: progress.device } };
	}
};

const lookOf = (state: RunState, step: SetupStep): { look: ConnectionLook; label: string } => {
	const last = state.steps.at(-1)?.step;
	if (state.done) return { look: "live", label: t("Done") };
	if (step !== last) return { look: "live", label: t("Done") };
	if (state.failed) return { look: "unreachable", label: t("Failed") };
	if (state.hostKey) return { look: "agent_down", label: t("Waiting for you") };
	return { look: "connecting", label: t("Running") };
};

/** Steps 3 to 7: the app at work, with the host key question when it comes. */
const Running = ({
	host,
	keyOf,
	state,
	onTrust,
	onAgain,
	onOpen,
}: {
	host: string;
	/** The machine whose host key it reads, when that's not `host`. */
	keyOf: string;
	state: RunState;
	onTrust: (ok: boolean) => void;
	onAgain: () => void;
	onOpen: (link: LinkId) => void;
}) => {
	const { t } = useI18n();

	const ended = state.done !== null || state.failed !== null;
	const left = STEPS.length - state.steps.filter((s) => s.step !== "pick").length;
	return (
		<div className="flex flex-col gap-4">
			<div className="flex flex-col gap-1">
				<Title>
					{state.done
						? t("{{host}} is set up", { host: host })
						: t("Setting up {{host}}", { host: host })}
				</Title>
				<Description>
					{state.done
						? t("Its agents and settings show under Devices.")
						: t("Nothing runs on the host until you trust its key.")}
				</Description>
			</div>

			<ol aria-label={t("Steps")} className="flex flex-col gap-2">
				{state.steps.map(({ step, phase }) => (
					<li
						key={step}
						className="flex items-center gap-2.5 text-[12.5px] text-foreground"
					>
						<ConnectionDot size={8} {...lookOf(state, step)} />
						<span className="selectable">
							{phaseText(phase, phase === "reading_host_key" ? keyOf : host)}
						</span>
					</li>
				))}
				{!ended && left > 0 ? (
					<li className="pl-[18px] text-xs text-muted-foreground">
						{t("{{countText}} more steps", {
							count: left,
							countText: formatNumber(left),
						})}
					</li>
				) : null}
			</ol>

			{state.hostKey ? (
				<div
					role="group"
					aria-label={t("Host key")}
					className="flex flex-col gap-2 rounded-lg border border-status-connecting/40 bg-status-connecting/10 p-3"
				>
					<span className="selectable text-[12.5px] text-foreground">
						{t(
							"{{host}} shows this {{type}} key. Check it matches the one the machine prints for ssh-keygen -lf on its host key.",
							{ host: state.hostKey.host, type: state.hostKey.key_type }
						)}
					</span>
					<div className="flex items-start gap-1">
						<code className="selectable min-w-0 flex-1 font-mono text-xs break-all text-foreground-strong">
							{state.hostKey.fingerprint}
						</code>
						<CopyIcon text={state.hostKey.fingerprint} />
					</div>
					<div className="flex justify-end gap-2">
						<button type="button" className={SECONDARY} onClick={() => onTrust(false)}>
							{t("Don't trust")}
						</button>
						<button type="button" className={PRIMARY} onClick={() => onTrust(true)}>
							{t("Trust this key")}
						</button>
					</div>
				</div>
			) : null}

			{state.hint ? (
				<div className="flex flex-col gap-2 rounded-lg border border-sidebar-border bg-subpanel p-3">
					<span className="selectable text-[12.5px] text-foreground">
						{hintText(state.hint.kind)}
					</span>
					<div className="flex items-center gap-2">
						<code
							title={state.hint.command}
							className="selectable min-w-0 flex-1 truncate font-mono text-xs text-foreground-strong"
						>
							{state.hint.command}
						</code>
						<CopyButton
							text={state.hint.command}
							label={t("Copy command")}
							icon={false}
							className={SECONDARY}
						/>
					</div>
				</div>
			) : null}

			{state.failed ? (
				<p role="alert" className="selectable text-xs text-status-error-text">
					{state.failed.reason}
				</p>
			) : null}

			<div className="flex justify-end gap-2">
				{state.failed ? (
					<button type="button" className={SECONDARY} onClick={onAgain}>
						{t("Try again")}
					</button>
				) : null}
				{state.done ? (
					<button
						type="button"
						className={SECONDARY}
						onClick={() => onOpen(state.done!.link)}
					>
						{t("Show {{host}}", { host: host })}
					</button>
				) : null}
				<Dialog.Close className={ended ? PRIMARY : SECONDARY}>
					{ended ? t("Done") : t("Close")}
				</Dialog.Close>
			</div>
		</div>
	);
};

type Stage = "pick" | "logins" | "run";

const Wizard = ({ onClose }: { onClose: () => void }) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const view = useView((s) => s.view);
	const [stage, setStage] = useState<Stage>("pick");
	const [hosts, setHosts] = useState<SetupHost[] | null>(null);
	const [host, setHost] = useState("");
	const [token, setToken] = useState("");
	const [run, setRun] = useState<RunState>(START);
	const accounts = [...view.tables.account.values()]
		.filter((a) => a.vendor === "claude" && !isLocalAccount(a))
		.map((a) => ({ id: a.id, label: accountName(a) }));
	const [account, setAccount] = useState<AccountId | null>(accounts[0]?.id ?? null);

	useEffect(() => {
		let gone = false;
		source.listSetupHosts().then(
			(found) => {
				if (!gone) setHosts(found);
			},
			(error: unknown) => {
				console.warn("Couldn't list the hosts to set up", error);
				if (!gone) setHosts([]);
			}
		);
		return () => {
			gone = true;
		};
	}, [source]);

	const start = () => {
		const chosen = accounts.find((a) => a.id === account);
		const secrets: SetupSecret[] =
			token.trim() !== "" && chosen
				? [
						{
							secret: "claude_token",
							origin: chosen.id,
							label: chosen.label,
							token: token.trim(),
						},
					]
				: [];
		const secret = token.trim();
		setRun(START);
		setStage("run");
		// The token goes in the request only, so the form lets go of it now.
		setToken("");
		source
			.setupDevice(host, secrets, (progress) =>
				setRun((state) =>
					follow(
						state,
						progress.type === "failed"
							? { ...progress, reason: scrub(progress.reason, secret) }
							: progress
					)
				)
			)
			.then(
				(result) => {
					if (result.status === "error") {
						const reason =
							result.error.kind === "failed"
								? scrub(result.error.message, secret)
								: t("Setup stopped");
						setRun((state) =>
							state.failed
								? state
								: {
										...state,
										failed: {
											step: state.steps.at(-1)?.step ?? "pick",
											reason,
										},
									}
						);
					}
				},
				(error: unknown) => {
					console.warn("Couldn't set the device up", error);
					setRun((state) => ({
						...state,
						failed: { step: "pick", reason: t("Couldn't reach the app. Try again.") },
					}));
				}
			);
	};

	const trust = (ok: boolean) => {
		const id = run.setup;
		setRun((state) => ({ ...state, hostKey: null }));
		if (id) void source.confirmHostKey(id, ok);
	};

	switch (stage) {
		case "pick":
			return (
				<Pick
					hosts={hosts}
					host={host}
					onHost={setHost}
					onNext={() => setStage("logins")}
				/>
			);
		case "logins":
			return (
				<Logins
					host={hostName(hosts, host)}
					accounts={accounts}
					account={account}
					token={token}
					onAccount={setAccount}
					onToken={setToken}
					onBack={() => setStage("pick")}
					onStart={start}
				/>
			);
		case "run":
			return (
				<Running
					host={hostName(hosts, host)}
					keyOf={keyHost(hosts, host)}
					state={run}
					onTrust={trust}
					onAgain={() => setStage("logins")}
					onOpen={(link) => {
						services.ui.getState().selectLink(link);
						onClose();
					}}
				/>
			);
	}
};

/** The sidebar's Add device button and the wizard it opens. */
export const AddDeviceButton = ({ className }: { className: string }) => {
	const { t } = useI18n();

	const [open, setOpen] = useState(false);
	const [opened, setOpened] = useState(0);
	return (
		<Dialog.Root open={open} onOpenChange={setOpen}>
			<button
				type="button"
				className={className}
				onClick={() => {
					setOpened((n) => n + 1);
					setOpen(true);
				}}
			>
				<svg
					aria-hidden
					width={13}
					height={13}
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					strokeWidth={2}
					strokeLinecap="round"
					strokeLinejoin="round"
					className="shrink-0"
				>
					<path d="M12 5v14M5 12h14" />
				</svg>
				{t("Add device")}
			</button>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[130] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-[12vh] left-1/2 z-[131] w-[min(520px,calc(100vw-32px))] -translate-x-1/2 rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					<Wizard key={opened} onClose={() => setOpen(false)} />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
