// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// For tests: one device section alone, over the sample source, fed the view
// as the device page feeds it.

import { render } from "@testing-library/react";
import type { ComponentType } from "react";

import { ServicesProvider, useView } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { DeviceId } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import type { ViewState } from "@maxhq/shared/view/replica";

/** What a device section takes. */
export type SectionProps = {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
};

/** Renders `Section` for `device` and gives the source it calls. */
export const renderSection = (
	Section: ComponentType<SectionProps>,
	device: DeviceId,
	name: string,
	source: DataSource = mockSource({ now: () => NOW, tick: false, scenario: "connected" })
) => {
	const Fed = () => {
		const view = useView((s) => s.view);
		return <Section device={device} name={name} live view={view} />;
	};
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={createUiStore()}>
				<Fed />
			</ServicesProvider>
		</FixedNow>
	);
	return { source };
};
