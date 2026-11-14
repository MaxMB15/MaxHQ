// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The SSH hosts a link tries, in order, with the one that worked last on
// this network marked. Each change goes to the app at once, which tries the
// list from the top the next time it connects.

import { useI18n } from "@maxhq/shared/i18n";

import { type FormEvent, useState } from "react";

import { useServices } from "@/app/services";
import { FIELD, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { LinkId } from "@maxhq/shared/bindings";
import { linkRoutes } from "@maxhq/shared/status/reach";
import type { ViewState } from "@maxhq/shared/view/replica";

/** The SSH hosts a link tries, to reorder, add to and take from. */
export const DeviceRoutes = ({ link, view }: { link: LinkId; view: ViewState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [draft, setDraft] = useState("");
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const found = view.tables.device_link.get(link);
	if (!found || found.route.type !== "ssh") return null;
	const routes = linkRoutes(found).flatMap((r) =>
		r.route.type === "ssh" ? [{ host: r.route.host, good: r.last_good_here }] : []
	);
	const hosts = routes.map((r) => r.host);

	const save = async (next: string[]) => {
		setBusy(true);
		setNote({ text: t("Saving"), error: false });
		try {
			const failure = failureOf(await source.setRoutes(link, next));
			setNote(failure ? { text: failure, error: true } : null);
			return failure === null;
		} catch (error) {
			console.warn("Couldn't save the routes", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
			return false;
		} finally {
			setBusy(false);
		}
	};

	const move = (from: number, to: number) => {
		const next = [...hosts];
		const [host] = next.splice(from, 1);
		next.splice(to, 0, host!);
		void save(next);
	};

	const add = async (event: FormEvent) => {
		event.preventDefault();
		const host = draft.trim();
		if (host === "") return;
		if (hosts.includes(host)) {
			setNote({ text: t("{{host}} is in the list already", { host: host }), error: true });
			return;
		}
		if (await save([...hosts, host])) setDraft("");
	};

	return (
		<Section
			title={t("Routes")}
			label={t("Routes to {{name}}", { name: found.name })}
			hint={note ? <StatusNote note={note} /> : t("MaxHQ tries them from the top")}
		>
			<ol aria-label={t("SSH hosts")} className="flex flex-col">
				{routes.map((route, i) => (
					<li
						key={route.host}
						className="border-t border-sidebar-border first:border-t-0"
					>
						<Row
							title={<code className="font-mono">{route.host}</code>}
							sub={route.good ? undefined : i === 0 ? t("Tried first") : undefined}
						>
							{route.good ? (
								<span className="inline-flex h-5 items-center rounded-[5px] border border-status-ready/40 bg-status-ready/10 px-[7px] text-[11.5px] text-status-ready">
									{t("Worked last on this network")}
								</span>
							) : null}
							<button
								type="button"
								className={SECONDARY}
								aria-label={t("Move {{host}} up", { host: route.host })}
								disabled={busy || i === 0}
								onClick={() => move(i, i - 1)}
							>
								{t("Up")}
							</button>
							<button
								type="button"
								className={SECONDARY}
								aria-label={t("Move {{host}} down", { host: route.host })}
								disabled={busy || i === routes.length - 1}
								onClick={() => move(i, i + 1)}
							>
								{t("Down")}
							</button>
							<button
								type="button"
								className={SECONDARY}
								aria-label={t("Remove {{host}}", { host: route.host })}
								disabled={busy || routes.length === 1}
								onClick={() => void save(hosts.filter((h) => h !== route.host))}
							>
								{t("Remove")}
							</button>
						</Row>
					</li>
				))}
			</ol>
			<form
				onSubmit={(event) => void add(event)}
				className="flex items-center gap-2.5 border-t border-sidebar-border px-4 py-3"
			>
				<input
					aria-label={t("SSH host")}
					placeholder={t("user@host, as ssh takes it")}
					value={draft}
					onChange={(event) => setDraft(event.target.value)}
					className={`${FIELD} min-w-0 flex-1`}
				/>
				<button type="submit" className={SECONDARY} disabled={busy || draft.trim() === ""}>
					{t("Add route")}
				</button>
			</form>
		</Section>
	);
};
