// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The shell rules Cursor saved in its own settings, device by device, each
// with Remove. Cursor runs a command that starts with a rule's words
// without asking MaxHQ, so each device that keeps any says so above them.
// A device reads its list when it's connected and the card opens.

import { useEffect, useState } from "react";

import { useServices, useView } from "@/app/services";
import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	afterRemove,
	cursorRuleDevices,
	type CursorRuleState,
	ruleState,
	ruleWords,
} from "@maxhq/shared/settings/cursor-rules";
import { isLive } from "@maxhq/shared/workspaces/state";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { ErrorCopy, Part, Row, SECONDARY, Section, StatusNote } from "./controls";
import type { Note } from "./result";

const QUIET = "px-4 py-3 text-xs text-muted-foreground";

const reachFailed = () => translate("Couldn't reach the device. Try again.");

/** What one device shows once it's connected: its rules, or why there are none. */
const Listed = ({ device, name }: { device: DeviceId; name: string }) => {
	useI18n();
	const { source } = useServices();
	const [state, setState] = useState<CursorRuleState>({ kind: "loading" });
	const [removing, setRemoving] = useState<string | null>(null);
	const [note, setNote] = useState<Note | null>(null);

	useEffect(() => {
		let gone = false;
		source.cursorRules(device, { kind: "list" }).then(
			(result) => {
				if (!gone) setState(ruleState(result));
			},
			(error: unknown) => {
				console.warn("Couldn't list the rules Cursor saved", error);
				if (!gone) setState({ kind: "failed", message: reachFailed() });
			}
		);
		return () => {
			gone = true;
		};
	}, [source, device]);

	const remove = async (rule: string) => {
		setRemoving(rule);
		setNote(null);
		try {
			const next = ruleState(await source.cursorRules(device, { kind: "remove", rule }));
			const done = afterRemove(state, next);
			setState(done.state);
			setNote(
				done.error === null
					? {
							text: translate(
								"Removed. Cursor agents running now keep it until they start again."
							),
							error: false,
						}
					: {
							text: done.error || translate("Couldn't change Cursor's settings."),
							error: true,
						}
			);
		} catch (error) {
			console.warn("Couldn't remove the rule Cursor saved", error);
			setNote({ text: reachFailed(), error: true });
		} finally {
			setRemoving(null);
		}
	};

	const body = () => {
		switch (state.kind) {
			case "loading":
				return <p className={QUIET}>{translate("Reading Cursor's settings")}</p>;
			case "too_old":
				return (
					<p className={QUIET}>
						{translate(
							"Update maxd on {{deviceName}} to see the rules Cursor saved there.",
							{
								deviceName: name,
							}
						)}
					</p>
				);
			case "failed": {
				const message = state.message || translate("Couldn't read Cursor's settings.");
				return (
					<p className="px-4 py-3 text-xs text-status-error-text">
						<span className="selectable">{message}</span>
						<ErrorCopy text={message} />
					</p>
				);
			}
			case "listed":
				return state.rules.length === 0 ? (
					<p className={QUIET}>{translate("Cursor saved no shell rules here.")}</p>
				) : (
					<>
						<p className="px-4 pb-2 text-xs text-secondary-foreground">
							{translate(
								"Cursor runs commands that start with these words without asking MaxHQ, in every project on this device. Always allow in an earlier MaxHQ saved them; MaxHQ now keeps those rules itself."
							)}
						</p>
						{state.rules.map((rule) => (
							<Row
								key={rule}
								title={
									<code className="selectable font-mono text-[12px]">
										{ruleWords(rule)}
									</code>
								}
							>
								<button
									type="button"
									className={SECONDARY}
									disabled={removing !== null}
									aria-label={translate("Remove {{rule}} on {{deviceName}}", {
										rule: ruleWords(rule),
										deviceName: name,
									})}
									onClick={() => void remove(rule)}
								>
									{removing === rule
										? translate("Removing")
										: translate("Remove")}
								</button>
							</Row>
						))}
					</>
				);
		}
	};

	return (
		<Part
			title={name}
			hint={
				state.kind === "listed" ? (
					<span className="font-mono text-[11px]">{state.path}</span>
				) : undefined
			}
			label={translate("On {{deviceName}}", { deviceName: name })}
		>
			{body()}
			{note ? <StatusNote note={note} className="block px-4 pb-3" /> : null}
		</Part>
	);
};

/** The Shell rules Cursor saved card: a part for each device with Cursor. */
export const CursorRules = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const devices = cursorRuleDevices(view);
	return (
		<Section
			title={translate("Shell rules Cursor saved")}
			hint={translate("Remove makes Cursor ask again")}
		>
			{devices.length === 0 ? (
				<p className={QUIET}>{translate("No device reports the Cursor CLI yet.")}</p>
			) : (
				devices.map(({ device, name }) =>
					isLive(view, device, now) ? (
						<Listed key={device} device={device} name={name} />
					) : (
						<Part
							key={device}
							title={name}
							label={translate("On {{deviceName}}", { deviceName: name })}
						>
							<p className={QUIET}>{translate("Not connected")}</p>
						</Part>
					)
				)
			)}
		</Section>
	);
};
