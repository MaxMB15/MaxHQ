// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The app keys in a device's settings: each key the device trusts, with
// how and when it came in and when it last proved, and a Remove for every
// key but this Mac's. Warnings come first when the device trusts a key
// this Mac didn't add or turned this Mac's key down, and a note when it
// took the first key offered without checking who offered it, with how to
// start over. This Mac's own page also makes a new key and lists the
// devices still on the old one.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { useServices } from "@/app/services";
import type { AppKey, DeviceId } from "@maxhq/shared/bindings";
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
import type { ViewState } from "@maxhq/shared/view/replica";

import { ConfirmButton, Row, Section, StatusNote } from "./controls";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const APP_KEY = "app-key";

/** How often the page asks again while devices move to a new key. */
const WHILE_MOVING_MS = 3000;

/** A warning at the top of the list. */
const Warning = ({ text }: { text: string }) => {
	useI18n();
	return <Row title={<span className="text-status-error-text">{text}</span>} />;
};

/** The app keys section, or nothing when the device lists none and has nothing to say. */
export const AppKeys = ({
	id,
	device,
	name,
	live,
	view,
}: {
	/** The card's element ID, for the settings search. */
	id: string;
	device: DeviceId;
	name: string;
	/** Whether the device's `maxd` answers, so it can take a removal. */
	live: boolean;
	view: ViewState;
}) => {
	useI18n();
	const { source } = useServices();
	const queries = useQueryClient();
	const now = useNow(60_000);
	const result = useQuery({
		queryKey: [APP_KEY],
		queryFn: () => source.appKey(),
		refetchInterval: (query) => {
			const data = query.state.data;
			return data?.status === "ok" && data.data.previous !== null ? WHILE_MOVING_MS : false;
		},
	});
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<{ key: string; note: Note } | null>(null);
	const state = result.data?.status === "ok" ? result.data.data : null;
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
		<Section
			title={translate("App keys")}
			id={id}
			label={translate("App keys for {{name}}", { name: name })}
			hint={translate("Apps that may do everything here")}
		>
			{refusal ? <Warning text={refusal} /> : null}
			{foreign ? <Warning text={foreign} /> : null}
			{first?.warning ? <Warning text={first.text} /> : null}
			{first && !first.warning ? <Row title={first.text} /> : null}
			{rows.map(({ key, own, sub }) => (
				<Row key={key.id} title={key.label} sub={sub}>
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
		</Section>
	);
};
