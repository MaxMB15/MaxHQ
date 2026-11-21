// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The projects list: each project with its checkout's folder and the
// devices that hold one, and Open, which shows the project's settings as
// a view of their own. A search lists only the projects it found.

import { useServices, useViewOf } from "@/app/services";
import type { EntityKind } from "@maxhq/shared/view/replica";
import { formatList, t as translate, useI18n } from "@maxhq/shared/i18n";
import { projectRows } from "@maxhq/shared/settings/project";

import { Row, SECONDARY, Section } from "../controls";
import { showPlace } from "../store";
import { Value } from "../value";
import type { Results } from "./find";

const KINDS: readonly EntityKind[] = ["project", "workspace", "device_link"];

/** The list's labels, for the settings search. */
export const PROJECTS_TERMS: readonly string[] = ["Open"];

/** The Projects card. */
export const ProjectsCard = ({ results }: { results: Results | null }) => {
	useI18n();
	const services = useServices();
	const view = useViewOf(KINDS);
	const shown = results?.projects ?? null;
	const rows = projectRows(view).filter((row) => shown === null || shown.has(row.project));
	return (
		<Section title={translate("Projects")}>
			{rows.length === 0 ? <Row title={translate("No projects yet")} /> : null}
			{rows.map((row) => (
				<Row
					key={row.project}
					title={row.name}
					sub={
						row.path || row.devices.length > 0 ? (
							<span className="inline-flex max-w-full min-w-0 flex-wrap items-center gap-x-1.5">
								{row.path ? (
									<Value text={row.path} what={translate("Folder")} />
								) : null}
								{row.devices.length > 0 ? (
									<span>
										{translate("on {{devices}}", {
											devices: formatList(row.devices),
										})}
									</span>
								) : null}
							</span>
						) : undefined
					}
				>
					<button
						type="button"
						className={SECONDARY}
						aria-label={translate("Open {{name}}", { name: row.name })}
						onClick={() =>
							showPlace(services, { kind: "project", project: row.project })
						}
					>
						{translate("Open")}
					</button>
				</Row>
			))}
		</Section>
	);
};
