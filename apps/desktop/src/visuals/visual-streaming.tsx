// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual still streaming in. Half a page would only draw broken, so it
// waits for its fence to close and says how much has come so far.

import { useI18n } from "@maxhq/shared/i18n";

import { cn } from "@/lib/utils";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import { visualBytes } from "@maxhq/shared/visuals/visual";

export const VisualStreaming = ({
	html,
	title,
	className,
}: {
	html: string;
	title: string;
	className?: string;
}) => {
	const { t } = useI18n();
	return (
		<div
			data-find-skip
			role="status"
			className={cn(
				"flex h-[34px] items-center gap-2 rounded-[10px] border border-dashed border-border px-3 text-[12px] text-muted-foreground select-none",
				className
			)}
		>
			<span aria-hidden className="text-(--pill-visual-text)">
				◆
			</span>
			<span className="min-w-0 truncate text-[13px] font-medium text-foreground">
				{title || t("Visual")}
			</span>
			<span className="shrink-0 motion-safe:animate-pulse">
				{t("Drawing when the block ends, {{size}} so far", {
					size: formatBytes(visualBytes(html)),
				})}
			</span>
		</div>
	);
};
