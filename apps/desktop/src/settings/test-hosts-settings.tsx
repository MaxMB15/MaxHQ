// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The test hosts page: each kind of test machine with the devices that
// could host it, a tick for each you pick, and once ticked what its `maxd`
// found there, with Build, Reset and Check again. A device whose `maxd`
// doesn't offer test hosts isn't listed.

import { useView } from "@/app/services";
import { kindLabel, TEST_KINDS } from "@maxhq/shared/test-hosts/hosts";
import { deviceLive, kindPlaces } from "@maxhq/shared/test-hosts/place";
import { useNow } from "@maxhq/shared/lib/clock";
import { PlaceBlock } from "@/test-hosts/places";

import { Section } from "./controls";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The page's headings, for the settings search: one for each kind of test machine. */
export const TEST_HOSTS_TERMS: readonly string[] = TEST_KINDS.map(kindLabel);

export const TestHostsSettingsPage = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const kinds = kindPlaces(view);
	if (kinds.every((k) => k.places.length === 0)) {
		return (
			<p className="text-[12.5px] text-muted-foreground">
				{translate("No device offers test hosts yet. They come with a newer maxd.")}
			</p>
		);
	}
	return (
		<>
			{kinds.map(({ kind, places }) => (
				<Section key={kind} title={kindLabel(kind)}>
					{places.length === 0 ? (
						<p className="px-4 py-3 text-xs text-muted-foreground">
							{translate("None of your devices can host it.")}
						</p>
					) : (
						places.map((place) => (
							<PlaceBlock
								key={place.device}
								place={place}
								title={place.name}
								view={view}
								live={deviceLive(view, place.device, now)}
								check
							/>
						))
					)}
				</Section>
			))}
			<p className="max-w-[64ch] text-xs text-muted-foreground">
				{translate(
					"Only agents on a device lease its test machines. For an agent elsewhere to test on one, start an agent on the device that hosts it."
				)}
			</p>
		</>
	);
};
