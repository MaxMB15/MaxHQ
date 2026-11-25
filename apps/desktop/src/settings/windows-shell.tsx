// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The shell a Windows device starts for `!` commands and the terminal
// drawer, in its Terminals card. It shows for Windows devices only, since
// other systems start your login shell, and only once the device's `maxd`
// keeps the choice. A save sends it with the device's other settings.

import type { DeviceId, WindowsShell } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import {
	picksWindowsShell,
	WINDOWS_SHELL_OPTIONS,
	windowsShellOf,
} from "@maxhq/shared/settings/windows-shell";
import type { ViewState } from "@maxhq/shared/view/replica";
import { Picker } from "@/new-agent/picker";

import { Row, StatusNote } from "./controls";
import type { Note } from "./result";

/** What Settings search finds the row by, on a device that shows it. */
export const windowsShellTerms = (): string[] => [translate("Shell for terminals")];

export const WindowsShellRow = ({
	view,
	device,
	why,
	value,
	note,
	onSave,
}: {
	view: ViewState;
	device: DeviceId;
	/** Why it can't change now, such as the device not answering, or null. */
	why: string | null;
	value: WindowsShell;
	note: Note | null;
	onSave: (shell: WindowsShell) => void;
}) => {
	useI18n();
	if (!picksWindowsShell(view, device)) return null;
	return (
		<Row
			title={translate("Shell for terminals")}
			sub={
				why ??
				translate(
					"The shell that ! commands and new terminals start. A shell you pick that isn't installed falls back to the automatic order. Applies to terminals opened after the change."
				)
			}
		>
			<StatusNote note={note} />
			<Picker
				label={translate("Shell for terminals")}
				value={value}
				options={WINDOWS_SHELL_OPTIONS}
				wrapDetails
				onChange={(picked) => onSave(windowsShellOf(picked))}
				disabled={why !== null}
			/>
		</Row>
	);
};
