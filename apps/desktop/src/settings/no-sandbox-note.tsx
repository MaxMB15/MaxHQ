// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The line that says agents on Windows run without a sandbox. Windows
// gives the agents' CLIs none, so what an agent's mode asks you first is
// all that stands between it and the PC. Settings says so once when the
// app runs on Windows, and each Windows device's New agents card says so
// wherever it shows.

import { cn } from "@/lib/utils";
import { onWindows } from "@/native/platform";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { wordingFor } from "@maxhq/shared/lib/os";

/** What the line says, as its key. */
export const NO_SANDBOX =
	"Agents on Windows run without a sandbox, but they still ask for permission as their mode says.";

/** Whether agents on a device whose system is `os` run without a sandbox. */
export const runsWithoutSandbox = (os: string | null | undefined): boolean =>
	wordingFor(os) === "windows";

/** The line, for a device whose system is `os`, or nothing when it has a sandbox. */
export const NoSandboxNote = ({
	os,
	className,
}: {
	os: string | null | undefined;
	className?: string;
}) => {
	useI18n();
	if (!runsWithoutSandbox(os)) return null;
	return (
		<p className={cn("selectable text-xs text-muted-foreground", className)}>
			{translate(NO_SANDBOX)}
		</p>
	);
};

/** The line Settings shows above its permissions, when the app runs on Windows. */
export const AppNoSandboxNote = () => (
	<NoSandboxNote os={onWindows() ? "windows" : null} className="px-1" />
);
