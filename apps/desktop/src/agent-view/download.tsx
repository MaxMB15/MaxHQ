// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Fetching a remote agent's files to this Mac. A file row or an attachment
// of an agent on another device gets a small button that downloads it into
// ~/Downloads/MaxHQ. The button shows how far it has got and cancels it,
// and once the file is here it shows it in Finder. The downloads live
// outside the rows, in a store the phone's Save uses too, so one keeps
// going while its row scrolls away.

import { Check, Download, FolderOpen, Pause, RotateCw } from "lucide-react";

import { useServices } from "@/app/services";
import { useComposerNative } from "@/composer/native";
import {
	cancelDownload,
	downloadPercent,
	failDownload,
	startDownload,
	useDownload,
} from "@maxhq/shared/agent-view/downloads";
import { cn } from "@/lib/utils";

import { useRow } from "@maxhq/shared/agent-view/context";
import { baseName } from "@maxhq/shared/agent-view/format";
import { Spinner } from "./icons";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

export { clearDownloads } from "@maxhq/shared/agent-view/downloads";

const HELD_TITLE = {
	get active() {
		return translate("Downloading. Click to cancel.");
	},
	get paused() {
		return translate("Paused, reconnecting");
	},
	get waiting() {
		return translate("Waiting for room on this Mac");
	},
} as const;

/**
 * The download button for a file a remote agent can see, as a path relative
 * to its folder or absolute inside it. Renders nothing for an agent on
 * this Mac, whose files are already here.
 */
export const DownloadButton = ({
	path,
	name = baseName(path),
	className,
}: {
	path: string;
	/** The name in the labels. The path's last part when unset. */
	name?: string;
	className?: string;
}) => {
	useI18n();
	const { agent, remote } = useRow();
	const { source } = useServices();
	const native = useComposerNative();
	const download = useDownload(agent, path);
	if (!remote) return null;

	const button = cn(
		"inline-flex h-[22px] min-w-[22px] shrink-0 cursor-pointer items-center justify-center gap-1 rounded-md px-1 text-[11px] text-muted-foreground tabular-nums hover:bg-accent hover:text-foreground",
		className
	);

	if (download === null) {
		return (
			<button
				type="button"
				aria-label={translate("Download {{name}}", { name: name })}
				title={translate("Download to this Mac")}
				onClick={() => void startDownload(source, agent, path)}
				className={button}
			>
				<Download size={13} strokeWidth={1.9} aria-hidden />
			</button>
		);
	}

	switch (download.state) {
		case "downloading": {
			const done = downloadPercent(download.sent, download.total);
			return (
				<button
					type="button"
					aria-label={translate("Cancel downloading {{name}}", { name: name })}
					title={HELD_TITLE[download.held ?? "active"]}
					onClick={() => cancelDownload(source, agent, path)}
					className={button}
				>
					{download.held ? (
						<Pause size={11} strokeWidth={2.2} aria-hidden />
					) : (
						<Spinner size={11} />
					)}
					{done !== null ? <span>{done}%</span> : null}
				</button>
			);
		}
		case "done": {
			const { revealPath } = native;
			if (!revealPath) {
				return (
					<span
						role="img"
						aria-label={translate("Downloaded {{name}}", { name: name })}
						title={download.path}
						className={cn(
							button,
							"cursor-default text-status-ready-text hover:bg-transparent"
						)}
					>
						<Check size={13} strokeWidth={2.2} aria-hidden />
					</span>
				);
			}
			return (
				<button
					type="button"
					aria-label={translate("Show {{name}} in Finder", { name: name })}
					title={download.path}
					onClick={() => {
						const landed = download.path;
						revealPath(landed).catch(() =>
							failDownload(
								agent,
								path,
								translate("Couldn't find {{baseName}} in Downloads.", {
									baseName: baseName(landed),
								})
							)
						);
					}}
					className={button}
				>
					<FolderOpen size={13} strokeWidth={1.9} aria-hidden />
				</button>
			);
		}
		case "failed":
			return (
				<>
					<span role="alert" className="sr-only">
						{download.reason}
					</span>
					<button
						type="button"
						aria-label={translate("Retry downloading {{name}}", { name: name })}
						title={download.reason}
						onClick={() => void startDownload(source, agent, path)}
						className={cn(button, "text-status-error-text")}
					>
						<RotateCw size={12} strokeWidth={2} aria-hidden />
					</button>
				</>
			);
	}
};
