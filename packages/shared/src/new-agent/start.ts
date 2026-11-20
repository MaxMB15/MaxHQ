// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the New agent form does on the Mac and on the phone, apart from
// drawing it: it reads each candidate's load every 5 seconds, it says where
// the mode came from until you pick one, and it starts the agent. Starting
// checks an Auto you didn't pick first, then sends the spec, and says why
// in words when the agent didn't start.

import { useEffect, useState } from "react";

import { t } from "../i18n";
import type { AgentId, AgentSpec, DeviceId, PermissionMode } from "../bindings";
import type { ActionResult, DataSource, DeviceLoad } from "../data/source";
import type { AgreedMode } from "../settings/auto-mode";
import { deviceDefault, MODE_LABEL } from "../settings/permission";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";

import { type Choices, type Resolved, startFailure } from "./spec";

/** How often the form reads the candidates' load. */
export const LOAD_POLL_MS = 5000;

/** The latest load of each device in `devices`, read again every 5 seconds. */
export const useLoads = (
	source: Pick<DataSource, "readLoad">,
	devices: readonly DeviceId[]
): DeviceLoad[] => {
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
				(error: unknown) => console.warn(t("Couldn't read the devices' load"), error)
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

/**
 * Where the mode came from, until you pick one: the device's default, or
 * Codex, which has no Plan and asks first instead. Null once you pick one
 * or before there's a device.
 */
export const modeNote = (view: ViewState, choices: Choices, r: Resolved): string | null => {
	if (choices.mode !== null || !r.device) return null;
	return r.mode === deviceDefault(view, r.device)
		? t("{{value1}} is the default on {{deviceName}}.", {
				value1: MODE_LABEL[r.mode],
				deviceName: deviceName(view, r.device),
			})
		: t("Codex has no plan mode, so it asks first.");
};

/** How starting went: the agent that started, or why not, null for a question you closed. */
export type StartOutcome = { readonly agent: AgentId } | { readonly failure: string | null };

/** The steps a start takes, which each app fills in. */
export type StartSteps = {
	/** The Auto check, with the mode it settles on. */
	readonly agree: (mode: PermissionMode) => Promise<AgreedMode>;
	/** Anything to send before the agent, such as the project's first settings. */
	readonly before?: (spec: AgentSpec) => Promise<void>;
	/** Sends the spec. */
	readonly start: (spec: AgentSpec) => Promise<ActionResult>;
};

/**
 * Starts the agent `spec` describes, in `mode` when the spec names none.
 * An Auto you didn't pick asks first, and Ask first instead starts it in
 * Ask first. Nothing here throws: what went wrong comes back in words.
 */
export const startNewAgent = async (
	spec: AgentSpec,
	mode: PermissionMode,
	steps: StartSteps
): Promise<StartOutcome> => {
	try {
		const asked = spec.permission ?? mode;
		const agreed = await steps.agree(asked);
		if (agreed.mode === null) return { failure: agreed.failure };
		if (agreed.failure) console.warn(agreed.failure);
		const sent = agreed.mode === asked ? spec : { ...spec, permission: agreed.mode };
		await steps.before?.(sent);
		const result = await steps.start(sent);
		if (result.status === "ok" && result.data.status === "started") {
			return { agent: result.data.agent };
		}
		return { failure: startFailure(result) };
	} catch (error) {
		console.warn(t("Couldn't start the agent"), error);
		return { failure: t("It didn't start. Try again.") };
	}
};
