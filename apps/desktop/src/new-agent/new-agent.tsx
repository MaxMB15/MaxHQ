// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The new-agent form: where it runs, which login and model, the mode it
// starts in, its branch in a container, and an optional title and first
// message. It opens from a project in the sidebar, the sidebar's button or
// Cmd+N, and once `maxd` says the agent started, its pane opens. Until you
// pick where it runs, the form reads each candidate's load every 5 seconds
// and puts the agent on the freest. While this Mac's maxd can't be reached
// and the agent would run here, it offers to start it.

import { Dialog } from "@base-ui/react/dialog";
import { type KeyboardEvent, useEffect, useMemo, useState } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import type { DeviceId, ProjectId } from "@maxhq/shared/bindings";
import type { DeviceLoad } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { CloneBanner } from "@/agent-view/clone-banner";
import { keyHint, type Shortcut } from "@/layout/shortcuts";
import { useFieldTyping } from "@/composer/typing-prefs";
import { cn } from "@/lib/utils";
import { LocalMaxdNotice } from "@/main-area/local-maxd";
import { deviceDefault, MODE_LABEL } from "@maxhq/shared/settings/permission";
import { agreedMode } from "@/settings/auto-consent";
import { sendFirstSettingsFor } from "@/settings/project";
import { deviceName } from "@maxhq/shared/view/select";

import { AGENT_FIELD, AgentPickers } from "./agent-pickers";
import { BranchField } from "./branch-field";
import {
	balanceCandidates,
	balanceNote,
	type Choices,
	emptyChoices,
	inContainer,
	pickBalanced,
	pickChoices,
	planStart,
	projectOptions,
	resolve,
	startFailure,
} from "@maxhq/shared/new-agent/spec";
import { closeNewAgent, newAgentOf, openNewAgent, useNewAgent } from "./store";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Label = ({ children }: { children: string }) => {
	useI18n();
	return <span className="text-[11.5px] font-[550] text-secondary-foreground">{children}</span>;
};

/** How often the form reads the candidates' load. */
const LOAD_POLL_MS = 5000;

/** The key that starts the agent from anywhere in the form. */
const START: Shortcut = { label: "↵", code: "Enter" };

/** The latest load of each device in `devices`, read again every 5 seconds. */
const useLoads = (devices: readonly DeviceId[]): DeviceLoad[] => {
	const { source } = useServices();
	const [loads, setLoads] = useState<DeviceLoad[]>([]);
	const key = devices.join(",");
	useEffect(() => {
		if (key === "") return;
		const asked = key.split(",");
		let live = true;
		const read = () =>
			source.readLoad(asked).then(
				(next) => {
					if (live) setLoads(next);
				},
				(error: unknown) =>
					console.warn(translate("Couldn't read the devices' load"), error)
			);
		void read();
		const timer = setInterval(() => void read(), LOAD_POLL_MS);
		return () => {
			live = false;
			clearInterval(timer);
		};
	}, [source, key]);
	return loads;
};

const Form = ({ project }: { project: ProjectId | null }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const [choices, setChoices] = useState<Choices>(() => {
		const empty = emptyChoices(project);
		return projectOptions(services.view.getState().view).length > 0
			? empty
			: { ...empty, place: "folder" };
	});
	const typing = useFieldTyping();
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);

	const candidates = useMemo(() => balanceCandidates(view, choices), [view, choices]);
	const loads = useLoads(candidates);
	// The clock here moves every 30 seconds, so a sample just in counts as now.
	const balanced = useMemo(
		() =>
			pickBalanced(
				view,
				candidates,
				loads,
				Math.max(now, ...loads.map((l) => l.received_at))
			),
		[view, candidates, loads, now]
	);
	const r = resolve(view, choices, balanced);
	const plan = planStart(view, choices, balanced);

	const pick = (patch: Partial<Choices>) => {
		setChoices((c) => pickChoices(view, c, patch, balanced));
		setFailure(null);
	};

	const start = async () => {
		if (!plan.ok || busy || !r.device) return;
		setBusy(true);
		setFailure(null);
		try {
			// An Auto you didn't pick, as the device's default, asks first.
			const mode = plan.spec.permission ?? r.mode;
			const agreed = await agreedMode(services, mode, r.device);
			if (agreed.mode === null) {
				setFailure(agreed.failure);
				setBusy(false);
				return;
			}
			if (agreed.failure) console.warn(agreed.failure);
			const spec =
				agreed.mode === mode ? plan.spec : { ...plan.spec, permission: agreed.mode };
			await sendFirstSettingsFor(services, spec);
			const result = await services.source.startAgent(spec);
			if (result.status === "ok" && result.data.status === "started") {
				closeNewAgent(services);
				openAgent(services, result.data.agent);
				return;
			}
			setFailure(startFailure(result));
		} catch (error) {
			console.warn(translate("Couldn't start the agent"), error);
			setFailure(translate("It didn't start. Try again."));
		}
		setBusy(false);
	};

	const startOnShortcut = (e: KeyboardEvent) => {
		if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			void start();
		}
	};

	// Says where the mode came from until you pick one.
	let modeNote: string | null = null;
	if (choices.mode === null && r.device) {
		modeNote =
			r.mode === deviceDefault(view, r.device)
				? translate("{{value1}} is the default on {{deviceName}}.", {
						value1: MODE_LABEL[r.mode],
						deviceName: deviceName(view, r.device),
					})
				: translate("Codex has no plan mode, so it asks first.");
	}

	return (
		<form
			onSubmit={(e) => {
				e.preventDefault();
				void start();
			}}
			className="flex flex-col gap-4"
		>
			<div className="flex flex-col gap-1">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{translate("New agent")}
				</Dialog.Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{translate("MaxHQ starts it and holds it, and its pane opens once it's up.")}
				</Dialog.Description>
			</div>

			<AgentPickers view={view} choices={choices} resolved={r} now={now} onPick={pick}>
				{r.balanced && r.device ? (
					<p className="-mt-2 text-[11.5px] text-muted-foreground">
						{balanceNote(view, r.device)}
						{translate(". Pick a place to choose yourself.")}
					</p>
				) : null}
				{modeNote ? (
					<p className="-mt-2 text-[11.5px] text-muted-foreground">{modeNote}</p>
				) : null}
				{r.workspace ? <CloneBanner workspace={r.workspace.id} /> : null}
				<LocalMaxdNotice device={r.device} onOpen={() => closeNewAgent(services)} />
			</AgentPickers>

			{r.project && r.workspace && inContainer(r.workspace) ? (
				<BranchField
					project={r.project}
					workspace={r.workspace.id}
					value={choices.branch}
					onChange={(branch) => pick({ branch })}
				/>
			) : null}

			<label className="flex flex-col gap-1.5">
				<Label>{translate("Title")}</Label>
				<input
					value={choices.title}
					onChange={(e) => pick({ title: e.target.value })}
					placeholder={translate(
						"Optional. Otherwise the CLI or the first message names it"
					)}
					{...typing}
					autoComplete="off"
					className={cn(AGENT_FIELD, "h-8")}
				/>
			</label>

			<label className="flex flex-col gap-1.5">
				<Label>{translate("First message")}</Label>
				<textarea
					value={choices.first}
					onChange={(e) => pick({ first: e.target.value })}
					onKeyDown={startOnShortcut}
					{...typing}
					placeholder={translate("Optional. What should it work on?")}
					rows={4}
					className={cn(AGENT_FIELD, "resize-none py-2 leading-normal")}
				/>
			</label>

			<div className="flex items-center gap-2">
				<p
					role="status"
					className={cn(
						"min-w-0 flex-1 text-[11.5px]",
						failure ? "text-status-error-text" : "text-muted-foreground"
					)}
				>
					{failure ?? (plan.ok ? null : plan.reason)}
				</p>
				<Dialog.Close className="inline-flex h-[30px] items-center rounded-[7px] border border-input bg-secondary px-3 text-[12.5px] text-foreground hover:bg-accent">
					{translate("Cancel")}
				</Dialog.Close>
				<button
					type="submit"
					title={translate("Start agent {{keyHint}}", { keyHint: keyHint(START) })}
					disabled={!plan.ok || busy}
					className="inline-flex h-[30px] items-center gap-2 rounded-[7px] bg-primary px-3 text-[12.5px] font-[650] text-primary-foreground shadow-lit enabled:hover:brightness-110 disabled:opacity-50"
				>
					{busy ? translate("Starting") : translate("Start agent")}
					<span className="text-[10.5px] font-medium opacity-70">{keyHint(START)}</span>
				</button>
			</div>
		</form>
	);
};

/** The form, and the Cmd+N shortcut that opens it. */
export const NewAgent = () => {
	useI18n();
	const services = useServices();
	const open = useNewAgent((s) => s.open);
	const opened = useNewAgent((s) => s.opened);
	const project = useNewAgent((s) => s.project);

	useEffect(() => {
		const onKey = (e: globalThis.KeyboardEvent) => {
			if (
				e.key.toLowerCase() !== "n" ||
				!(e.metaKey || e.ctrlKey) ||
				e.shiftKey ||
				e.altKey
			) {
				return;
			}
			e.preventDefault();
			// Pressing it again with the form open keeps what you typed.
			if (!newAgentOf(services).getState().open) openNewAgent(services);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [services]);

	return (
		<Dialog.Root
			open={open}
			onOpenChange={(next) => {
				if (!next) closeNewAgent(services);
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-[12vh] left-1/2 z-[121] w-[min(560px,calc(100vw-32px))] -translate-x-1/2 rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					<Form key={opened} project={project} />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
