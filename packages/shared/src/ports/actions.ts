// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a port's buttons do: forward it here, stop the forward, or open it
// in the browser. Each port tracks its forward on its way and why its last
// action failed. The desktop and the phone both use them. The phone also
// hears which forwards it opened and closed, since the view lists every
// computer's forwards and can't tell it which are its own.

import { formatNumber, t } from "../i18n";

import { useState } from "react";

import type { ForwardId, ListeningPort, PortId } from "../bindings";
import type { DataSource, ForwardResult } from "../data/source";
import { errorText } from "../timeline/store";

const reasonOf = (error: unknown): string =>
	error instanceof Error && error.message ? ` ${error.message}` : "";

/** The forward that opened, and the port it listens on here. */
export type OpenedForward = Extract<ForwardResult, { status: "ok" }>["data"];

/** What a client hears as its forwards open and close. */
export type ForwardHooks = {
	/** A forward to `port` opened, listening here on `opened.local_port`. */
	readonly forwarded?: (port: ListeningPort, opened: OpenedForward) => void;
	/** The forward to `port` closed. */
	readonly stopped?: (port: ListeningPort) => void;
};

/** The buttons' actions for a set of ports, and how they're going. */
export type PortActions = {
	/** Whether a forward to the port is opening or closing. */
	readonly busy: (port: PortId) => boolean;
	/** Why the last action on the port failed, if it did. */
	readonly failure: (port: PortId) => string | null;
	/** Opens a forward to the port from here. */
	readonly forward: (port: ListeningPort) => void;
	/** Closes the port's forward. */
	readonly stop: (port: ListeningPort, forward: ForwardId) => void;
	/** Opens http://localhost at a port here in the browser. */
	readonly open: (port: ListeningPort, local: number) => void;
};

/** Actions on ports through `source`, each with its own progress and failure. */
export const usePortActions = (
	source: Pick<DataSource, "openForward" | "closeForward" | "openLocalhost">,
	hooks: ForwardHooks = {}
): PortActions => {
	const [busy, setBusy] = useState<ReadonlySet<PortId>>(() => new Set());
	const [failures, setFailures] = useState<ReadonlyMap<PortId, string>>(() => new Map());

	const hold = (port: PortId, held: boolean) =>
		setBusy((was) => {
			const next = new Set(was);
			if (held) next.add(port);
			else next.delete(port);
			return next;
		});

	// Each action resolves with why it failed, or null, and never rejects.
	// Opening the browser doesn't hold up the port's other buttons.
	const run = (port: PortId, action: () => Promise<string | null>, holds: boolean) => {
		if (holds) hold(port, true);
		setFailures((was) => {
			const next = new Map(was);
			next.delete(port);
			return next;
		});
		void action().then((failure) => {
			if (holds) hold(port, false);
			if (failure !== null) setFailures((was) => new Map(was).set(port, failure));
		});
	};

	return {
		busy: (port) => busy.has(port),
		failure: (port) => failures.get(port) ?? null,
		forward: (port) =>
			run(
				port.id,
				() =>
					source.openForward(port.id).then(
						(result) => {
							if (result.status === "ok") {
								hooks.forwarded?.(port, result.data);
								return null;
							}
							return t("Couldn't forward port {{port}}. {{value2}}", {
								port: formatNumber(port.port, { useGrouping: false }),
								value2: errorText(result.error),
							});
						},
						(error: unknown) =>
							t("Couldn't forward port {{port}}.{{value2}}", {
								port: formatNumber(port.port, { useGrouping: false }),
								value2: reasonOf(error),
							})
					),
				true
			),
		stop: (port, forward) =>
			run(
				port.id,
				() =>
					source.closeForward(forward).then(
						() => {
							hooks.stopped?.(port);
							return null;
						},
						(error: unknown) =>
							t("Couldn't stop forwarding {{port}}.{{value2}}", {
								port: formatNumber(port.port, { useGrouping: false }),
								value2: reasonOf(error),
							})
					),
				true
			),
		open: (port, local) =>
			run(
				port.id,
				() =>
					source.openLocalhost(local).then(
						() => null,
						(error: unknown) =>
							t("Couldn't open localhost:{{local}}.{{value2}}", {
								local: formatNumber(local, { useGrouping: false }),
								value2: reasonOf(error),
							})
					),
				false
			),
	};
};
