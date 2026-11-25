// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The shell a Windows device starts for `!` commands and the terminal
// drawer. Automatic takes Git Bash when Git for Windows is installed, else
// PowerShell 7, else Windows PowerShell 5.1, which comes with Windows. A
// shell you pick that isn't installed falls back to that order. Other
// systems start your login shell, so the choice shows for Windows devices
// only, and a `maxd` from before WINDOWS_SHELL_SINCE picks on its own.

import { type DeviceId, WINDOWS_SHELL_SINCE, type WindowsShell } from "../bindings";
import { t } from "../i18n";
import type { Option } from "../new-agent/spec";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** Whether `device` is a Windows device whose `maxd` keeps the choice. */
export const picksWindowsShell = (view: ViewState, device: DeviceId): boolean => {
	if (view.tables.device.get(device)?.os !== "windows") return false;
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= WINDOWS_SHELL_SINCE;
};

/** The shells to pick from, Automatic first. */
export const WINDOWS_SHELL_OPTIONS: readonly (Option & { readonly value: WindowsShell })[] = [
	{
		value: "auto",
		get label() {
			return t("Automatic");
		},
		get detail() {
			return t("Git Bash when installed, else PowerShell 7, else Windows PowerShell 5.1");
		},
	},
	{
		value: "git_bash",
		get label() {
			return t("Git Bash");
		},
		get detail() {
			return t("From Git for Windows");
		},
	},
	{
		value: "pwsh",
		get label() {
			return t("PowerShell 7");
		},
	},
	{
		value: "power_shell",
		get label() {
			return t("Windows PowerShell 5.1");
		},
		get detail() {
			return t("Comes with Windows");
		},
	},
];

/** `value` as a shell to pick, with anything unknown read as Automatic. */
export const windowsShellOf = (value: string): WindowsShell =>
	WINDOWS_SHELL_OPTIONS.find((option) => option.value === value)?.value ?? "auto";
