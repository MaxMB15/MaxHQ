// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The App keys card in Permissions, a part for each device: each key the
// device trusts, with how and when it came in and when it last proved, and
// a Remove for every key but this Mac's. Warnings come first when the
// device trusts a key this Mac didn't add or turned this Mac's key down,
// and a note when it took the first key offered without checking who
// offered it, with how to start over. This Mac's part also makes a new key
// and lists the devices still on the old one.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { useServices, useViewOf } from "@/app/services";
import type { AppKey, AppKeyState, DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	appKeyRows,
	firstKeyNote,
	foreignKeyWarning,
	isThisMac,
	keyStateNote,
	refusalNote,
	rotationNote,
} from "@maxhq/shared/settings/app-keys";
import { accountDevices } from "@maxhq/shared/settings/accounts";
import { deviceLive } from "@maxhq/shared/test-hosts/place";
import type { EntityKind, ViewState } from "@maxhq/shared/view/replica";

import { AppKeyHomeRow } from "./app-key-home";
import { ConfirmButton, Part, Row, Section, StatusNote } from "./controls";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const APP_KEY = "app-key";

/** How often the card asks again while devices move to a new key. */
const WHILE_MOVING_MS = 3000;

/** What the card reads from the view. */
const KINDS: readonly EntityKind[] = ["device", "device_link", "network", "app_key"];

/** The card's labels, for the settings search. */
export const APP_KEYS_TERMS: readonly string[] = ["This Mac's app key", "Make a new app key"];

/** A warning at the top of a device's part. */
const Warning = ({ text }: { text: string }) => {
	useI18n();
	return <Row title={<span className="selectable text-status-error-text">{text}</span>} />;
};

/** This Mac's app key, as maxd keeps it, asked again while devices move to a new one. */
const useAppKey = (): AppKeyState | null => {
	const { source } = useServices();
	const result = useQuery({
		queryKey: [APP_KEY],
		queryFn: () => source.appKey(),
		refetchInterval: (query) => {
			const data = query.state.data;
			return data?.status === "ok" && data.data.previous !== null ? WHILE_MOVING_MS : false;
		},
	});
	return result.data?.status === "ok" ? result.data.data : null;
};

/** One device's part, or nothing when the device lists no keys and has nothing to say. */
const DeviceKeys = ({
	device,
	name,
	view,
	state,
}: {
	device: DeviceId;
	name: string;
	view: ViewState;
	state: AppKeyState | null;
}) => {
	useI18n();
	const { source } = useServices();
	const queries = useQueryClient();
	const now = useNow(60_000);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<{ key: string; note: Note } | null>(null);
	// Whether the device's `maxd` answers, so it can take a removal.
	const live = deviceLive(view, device, now);
	const here = isThisMac(view, device) && state !== null;
	const rows = appKeyRows(view, device, state, now);
	const refusal = refusalNote(view, device, state);
	const foreign = foreignKeyWarning(view, device, state, now);
	const first = firstKeyNote(view, device, state);
	if (rows.length === 0 && refusal === null && !here) return null;

	const act = async (key: string, what: string, run: () => Promise<string | null>) => {
		setBusy(true);
		setNote(null);
		try {
			const failure = await run();
			if (failure) setNote({ key, note: { text: failure, error: true } });
		} catch (error) {
			console.warn(translate("Couldn't {{what}}", { what: what }), error);
			setNote({ key, note: { text: translate("Couldn't do it. Try again."), error: true } });
		} finally {
			setBusy(false);
			await queries.invalidateQueries({ queryKey: [APP_KEY] });
		}
	};
	const remove = (key: AppKey) =>
		act(key.id, translate("remove an app key"), async () =>
			failureOf(await source.removeAppKey(device, key.public_key))
		);
	const rotate = () =>
		act("rotate", translate("make a new app key"), async () =>
			failureOf(await source.rotateAppKey())
		);
	const kept = state && here ? keyStateNote(state) : null;

	return (
		<Part title={name} label={translate("App keys for {{name}}", { name: name })}>
			{refusal ? <Warning text={refusal} /> : null}
			{foreign ? <Warning text={foreign} /> : null}
			{first?.warning ? <Warning text={first.text} /> : null}
			{first && !first.warning ? (
				<Row title={<span className="selectable">{first.text}</span>} />
			) : null}
			{rows.map(({ key, own, sub }) => (
				<Row key={key.id} title={<span className="selectable">{key.label}</span>} sub={sub}>
					<StatusNote note={note?.key === key.id ? note.note : null} />
					{own ? null : (
						<ConfirmButton
							label={translate("Remove")}
							question={translate("Stop {{name}} trusting it?", { name: name })}
							confirm={translate("Remove")}
							disabled={!live || busy}
							onConfirm={() => void remove(key)}
						/>
					)}
				</Row>
			))}
			{state && here ? (
				<Row title={translate("This Mac's app key")} sub={rotationNote(view, state)}>
					<StatusNote note={note?.key === "rotate" ? note.note : null} />
					<ConfirmButton
						label={translate("Make a new app key")}
						question="Move every device to a new key?"
						confirm={translate("Make it")}
						disabled={busy || state.previous !== null}
						onConfirm={() => void rotate()}
					/>
				</Row>
			) : null}
			{kept ? <Row title={kept} /> : null}
			{here ? <AppKeyHomeRow /> : null}
		</Part>
	);
};

/** The App keys card: the keys each device trusts, this Mac's first. */
export const AppKeysCard = () => {
	useI18n();
	const view = useViewOf(KINDS);
	const state = useAppKey();
	return (
		<Section title={translate("App keys")} hint={translate("Apps that may do everything here")}>
			{accountDevices(view).map(({ device, name }) => (
				<DeviceKeys key={device} device={device} name={name} view={view} state={state} />
			))}
		</Section>
	);
};
