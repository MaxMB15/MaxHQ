// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a port's buttons do: forward it here, stop the forward, or open it
// in the browser. Each port tracks its forward on its way and why its last
// action failed.

import { t, formatNumber } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import type { ForwardId, ListeningPort, PortId } from "@maxhq/shared/bindings";
import { errorText } from "@maxhq/shared/timeline/store";

const reasonOf = (error: unknown): string =>
	error instanceof Error && error.message ? ` ${error.message}` : "";

/** The buttons' actions for a set of ports, and how they're going. */
export type PortActions = {
	/** Whether a forward to the port is opening or closing. */
	readonly busy: (port: PortId) => boolean;
	/** Why the last action on the port failed, if it did. */
	readonly failure: (port: PortId) => string | null;
	/** Opens a forward to the port from this Mac. */
	readonly forward: (port: ListeningPort) => void;
	/** Closes the port's forward. */
	readonly stop: (port: ListeningPort, forward: ForwardId) => void;
	/** Opens http://localhost at a port on this Mac in the browser. */
	readonly open: (port: ListeningPort, local: number) => void;
};

/** Actions on ports, each with its own progress and failure. */
export const usePortActions = (): PortActions => {
	const { source } = useServices();
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
						(result) =>
							result.status === "ok"
								? null
								: t("Couldn't forward port {{port}}. {{value2}}", {
										port: formatNumber(port.port, { useGrouping: false }),
										value2: errorText(result.error),
									}),
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
						() => null,
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
