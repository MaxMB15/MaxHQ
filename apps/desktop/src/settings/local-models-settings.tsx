// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Local models page: a device picker, then that device's hardware and
// the GPU memory its models can use, the model new Local agents start on,
// the catalog of models it can download and run, the servers its Local
// agents use, and the settings for them. A device that can't run models
// itself says why and points to a server URL. Its `maxd` keeps the
// settings, and a save sends all of the device's settings, so changing one
// keeps the others as the device has them.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { Picker } from "@/new-agent/picker";
import type { DeviceId, KvCacheType } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { localSettingsOf } from "@maxhq/shared/local-models/models";
import { withDefaultLocalModel } from "@maxhq/shared/local-models/select";
import {
	contextOptions,
	defaultModelOptions,
	defaultModelValue,
	deviceState,
	hardwareText,
	kvTypeOptions,
	localDeviceOptions,
	pageDevice,
	parseIdleMinutes,
	pickOf,
} from "@maxhq/shared/local-models/settings-page";
import { gpuLimitText, runtimeSupportText, tooSlowLabel } from "@maxhq/shared/local-models/wording";
import {
	type DeviceFields as Fields,
	deviceSettingsOf,
	sameDeviceFields as same,
	settingsWhy,
} from "@maxhq/shared/settings/device-settings";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName, linkForDevice } from "@maxhq/shared/view/select";

import { Row, SECONDARY, Section, StatusNote } from "./controls";
import { NumberField } from "./device-settings";
import { ModelsSection } from "./local-models-catalog";
import { ServersSection } from "./local-models-servers";
import { failureOf, type Note } from "./result";
import { showPlace, useSettings } from "./store";

/** The page's headings and labels, for the settings search. */
export const LOCAL_MODELS_TERMS: readonly string[] = [
	"Device",
	"Hardware",
	"GPU",
	"Default model",
	"Models",
	"Download",
	"Servers",
	"Linked devices can use this Mac's models",
	"Add server URL",
	"Token, if the server asks for one",
	"Local agent settings",
	"Unload models after",
	"KV cache",
	"Context size",
	"Fewer tools for local agents",
];

type Part = "default" | "linked" | "idle" | "kv" | "context" | "tools";

/**
 * A device's settings as the page shows them, and a save that sends them
 * all with one part changed. What you just picked shows until the view moves
 * on from what it had then.
 */
const useDeviceSave = (device: DeviceId, view: ViewState) => {
	const { source } = useServices();
	const current = deviceSettingsOf(view, device);
	const [pending, setPending] = useState<{ fields: Fields; from: Fields } | null>(null);
	const [note, setNote] = useState<{ part: Part; note: Note } | null>(null);
	const shown = pending && same(pending.from, current) ? pending.fields : current;

	const save = async (part: Part, fields: Fields) => {
		if (same(fields, shown)) return;
		setPending({ fields, from: current });
		setNote({ part, note: { text: translate("Saving"), error: false } });
		try {
			const failure = failureOf(await source.setDeviceSettings({ device, ...fields }));
			if (failure) {
				setPending(null);
				setNote({ part, note: { text: failure, error: true } });
			} else {
				setNote({ part, note: { text: translate("Saved"), error: false } });
			}
		} catch (error) {
			console.warn(translate("Couldn't save the device settings"), error);
			setPending(null);
			setNote({
				part,
				note: { text: translate("Couldn't save it. Try again."), error: true },
			});
		}
	};
	const saveLocal = (part: Part, change: Partial<Fields["local_models"]>) =>
		save(part, { ...shown, local_models: { ...shown.local_models, ...change } });
	const noteFor = (part: Part) => (note?.part === part ? note.note : null);
	const reject = (part: Part, reason: string) =>
		setNote({ part, note: { text: reason, error: true } });
	return { shown, save, saveLocal, noteFor, reject };
};

/** What a device's page shows once it reports its local models. */
const DeviceBody = ({ device, view }: { device: DeviceId; view: ViewState }) => {
	useI18n();
	const name = deviceName(view, device);
	const live = linkForDevice(view, device)?.state.state === "connected";
	const why = settingsWhy(view, device, name, live);
	const state = deviceState(view, device);
	const { shown, save, saveLocal, noteFor, reject } = useDeviceSave(device, view);
	const [adding, setAdding] = useState(false);

	if (state.state !== "ready") {
		return (
			<Section title={translate("Local models")}>
				<Row
					title={name}
					sub={
						why.local ??
						translate("Waiting for {{name}} to report its hardware", { name })
					}
				/>
			</Section>
		);
	}

	const local = state.local;
	const managed = local.runtime.support === "managed";
	const settings = localSettingsOf(shown.local_models);
	const support = runtimeSupportText(local.runtime.support);

	return (
		<>
			<Section
				title={translate("Hardware")}
				label={translate("Hardware of {{name}}", { name })}
			>
				<Row
					title={hardwareText(local.hardware)}
					sub={gpuLimitText(local.hardware) ?? undefined}
				/>
				{support ? (
					<Row
						title={
							local.runtime.support === "no_gpu"
								? tooSlowLabel()
								: translate("Runs no models itself")
						}
						sub={support}
					>
						<button
							type="button"
							className={SECONDARY}
							disabled={why.local !== null || adding}
							onClick={() => setAdding(true)}
						>
							{translate("Add server URL")}
						</button>
					</Row>
				) : null}
			</Section>
			<Section
				title={translate("Default model")}
				label={translate("Default model on {{name}}", { name })}
			>
				<Row
					title={translate("Default model")}
					sub={
						why.local ??
						translate(
							"New Local agents on {{name}} start on it. Automatic picks the best model that's ready.",
							{ name }
						)
					}
				>
					<StatusNote note={noteFor("default")} />
					<Picker
						label={translate("Default model")}
						value={defaultModelValue(settings)}
						options={defaultModelOptions(view, device, settings)}
						onChange={(value) =>
							void save(
								"default",
								withDefaultLocalModel(shown, pickOf(view, device, value))
							)
						}
						disabled={why.local !== null}
					/>
				</Row>
			</Section>
			<ModelsSection device={device} view={view} disabled={why.local} />
			<ServersSection
				device={device}
				view={view}
				disabled={why.local}
				adding={adding}
				onAdding={setAdding}
				sharing={
					managed
						? {
								on: settings.linked_devices,
								note: noteFor("linked"),
								onChange: (on) => void saveLocal("linked", { linked_devices: on }),
							}
						: null
				}
			/>
			<Section
				title={translate("Local agent settings")}
				label={translate("Local agent settings on {{name}}", { name })}
			>
				{managed ? (
					<>
						<Row
							title={translate("Unload models after")}
							sub={
								why.local ??
								translate(
									"maxd unloads a model no agent used for this long, to free its memory. 0 keeps it loaded."
								)
							}
						>
							<StatusNote note={noteFor("idle")} />
							<NumberField
								label={translate("Unload models after")}
								value={settings.idle_unload_minutes}
								unit={translate("minutes")}
								disabled={why.local !== null}
								onCommit={(text) => {
									const parsed = parseIdleMinutes(text);
									if (!parsed.ok) return reject("idle", parsed.reason);
									void saveLocal("idle", { idle_unload_minutes: parsed.minutes });
								}}
							/>
						</Row>
						<Row
							title={translate("KV cache")}
							sub={
								why.local ??
								translate(
									"How the server keeps the context it has read. Applies the next time a model loads."
								)
							}
						>
							<StatusNote note={noteFor("kv")} />
							<Picker
								label={translate("KV cache")}
								value={settings.kv_type}
								options={kvTypeOptions()}
								onChange={(value) =>
									void saveLocal("kv", { kv_type: value as KvCacheType })
								}
								disabled={why.local !== null}
							/>
						</Row>
						<Row
							title={translate("Context size")}
							sub={
								why.local ??
								translate(
									"How much a Local agent can keep in mind. More takes more memory. Applies the next time a model loads."
								)
							}
						>
							<StatusNote note={noteFor("context")} />
							<Picker
								label={translate("Context size")}
								value={String(settings.context)}
								options={contextOptions(settings.context)}
								onChange={(value) =>
									void saveLocal("context", { context: Number(value) })
								}
								disabled={why.local !== null}
							/>
						</Row>
					</>
				) : null}
				<Row
					title={translate("Fewer tools for local agents")}
					sub={
						why.local ??
						translate(
							"Local agents on your own servers get only the tools they use most, so a small model has less to read and answers sooner. Cloud services always get every tool."
						)
					}
				>
					<StatusNote note={noteFor("tools")} />
					<Switch
						aria-label={translate("Fewer tools for local agents")}
						checked={settings.fewer_tools}
						disabled={why.local !== null}
						onCheckedChange={(on) => void saveLocal("tools", { fewer_tools: on })}
					/>
				</Row>
			</Section>
		</>
	);
};

/**
 * The Local models settings page. It opens on the device the page names,
 * else this Mac, else the first device with local models, and the picker at
 * its top shows another.
 */
export const LocalModelsSettingsPage = () => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const wanted = useSettings((s) =>
		s.detail?.kind === "local-models" ? s.detail.device : undefined
	);
	const device = pageDevice(view, wanted);

	if (!device) {
		return (
			<p className="px-1 py-6 text-center text-[12.5px] text-muted-foreground">
				{translate("Link a device to run local models on it.")}
			</p>
		);
	}
	return (
		<div className="flex flex-col gap-4">
			<div className="flex items-center gap-2.5">
				<Picker
					label={translate("Device")}
					value={device}
					options={localDeviceOptions(view)}
					onChange={(next) => showPlace(services, { kind: "local-models", device: next })}
				/>
			</div>
			<DeviceBody key={device} device={device} view={view} />
		</div>
	);
};
