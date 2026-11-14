// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Updates page: this app's version and where its update stands, the
// switch that lets it look for a newer one on its own, and Check now. A
// copy that doesn't update itself, such as one built from source or run
// from its disk image, says why here. The switch is kept on this Mac.

import { Switch } from "@/components/ui/switch";
import { OutLink } from "@/components/out-link";
import {
	checkNow,
	installing,
	releaseNotesUrl,
	setAutoCheck,
	statusText,
	updateNow,
	useUpdates,
} from "@/updates/store";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { PRIMARY, Row, SECONDARY, Section, StatusNote } from "./controls";

/** The page's headings and labels, for the settings search. */
export const UPDATES_TERMS: readonly string[] = [
	"Check for updates automatically",
	"Check now",
	"Update now",
	"Version",
];

/** The Updates page. */
export const UpdatesSettingsPage = () => {
	useI18n();
	const update = useUpdates((s) => s.update);
	const error = useUpdates((s) => s.error);
	const status = update?.status;
	const offered = status !== undefined && status.state !== "unavailable";
	const busy = status?.state === "checking" || installing(status);
	const notes =
		status?.state === "available"
			? releaseNotesUrl(update?.release_notes, status.version)
			: null;
	return (
		<Section title={translate("Updates")}>
			<Row
				title={
					update
						? translate("Version {{version}}", { version: update.version })
						: translate("Version")
				}
				sub={
					<>
						<span role="status">
							{status ? statusText(status) : translate("Checking for updates…")}
						</span>
						{error ? (
							<StatusNote note={{ text: error, error: true }} className="block" />
						) : null}
					</>
				}
			>
				{status?.state === "available" ? (
					<>
						{notes ? (
							<OutLink href={notes} className="text-[12.5px]">
								{translate("What's new")}
							</OutLink>
						) : null}
						<button type="button" className={PRIMARY} onClick={() => void updateNow()}>
							{translate("Update now")}
						</button>
					</>
				) : null}
				<button
					type="button"
					className={SECONDARY}
					disabled={!offered || busy}
					onClick={() => void checkNow()}
				>
					{translate("Check now")}
				</button>
			</Row>
			<Row
				title={translate("Check for updates automatically")}
				sub={translate("A little after MaxHQ opens, then every few hours")}
			>
				<Switch
					aria-label={translate("Check for updates automatically")}
					checked={update?.prefs.auto_check ?? true}
					disabled={!offered}
					onCheckedChange={(on) => void setAutoCheck(on)}
				/>
			</Row>
		</Section>
	);
};
