// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The device links in a device's settings: each other device it links to,
// where the link stands, and a button that turns the pair off or allows
// it again. A device that waits hears once it's back.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { useServices } from "@/app/services";
import type { DeviceId, PeerLink } from "@maxhq/shared/bindings";
import { linkAction, linkSub } from "@maxhq/shared/settings/device-links";

import { Row, SECONDARY, Section, StatusNote } from "./controls";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const LINKS = "device-links";

/** The device links section, or nothing when there's no other device. */
export const DeviceLinks = ({ device, name }: { device: DeviceId; name: string }) => {
	useI18n();
	const { source } = useServices();
	const queries = useQueryClient();
	const links = useQuery({
		queryKey: [LINKS, device],
		queryFn: () => source.listDeviceLinks(device),
	});
	const [busy, setBusy] = useState<DeviceId | null>(null);
	const [note, setNote] = useState<{ peer: DeviceId; note: Note } | null>(null);
	const result = links.data;
	if (!result || (result.status === "ok" && result.data.length === 0)) return null;

	const press = async (link: PeerLink) => {
		const { on } = linkAction(link);
		setBusy(link.peer);
		setNote(null);
		try {
			const failure = failureOf(await source.setDeviceLink(device, link.peer, on));
			if (failure) setNote({ peer: link.peer, note: { text: failure, error: true } });
		} catch (error) {
			console.warn(translate("Couldn't change a device link"), error);
			setNote({
				peer: link.peer,
				note: { text: translate("Couldn't change it. Try again."), error: true },
			});
		} finally {
			setBusy(null);
			// Both devices' lists change.
			await queries.invalidateQueries({ queryKey: [LINKS] });
		}
	};

	return (
		<Section
			title={translate("Device links")}
			label={translate("Device links for {{name}}", { name: name })}
			hint={translate("Lets agents here reach agents on the others")}
		>
			{result.status === "error" ? (
				<Row title={translate("Device links")} sub={failureOf(result)} />
			) : (
				result.data.map((link) => (
					<Row key={link.peer} title={link.name} sub={linkSub(link)}>
						<StatusNote note={note?.peer === link.peer ? note.note : null} />
						<button
							type="button"
							className={SECONDARY}
							aria-label={`${linkAction(link).label} ${link.name}`}
							disabled={busy !== null}
							onClick={() => void press(link)}
						>
							{linkAction(link).label}
						</button>
					</Row>
				))
			)}
		</Section>
	);
};
