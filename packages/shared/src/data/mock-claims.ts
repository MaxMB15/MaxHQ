// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The calls that answer whether a link is the device its hello named, over
// the sample entities, the way the app answers them: the same device pins
// the link there, a different one keeps it cut off, and forgetting a link's
// identity asks again when another link knows the device.

import type { DeviceLink, LinkClaimDetails, LinkId } from "../bindings";
import { linkRoutes } from "../status/reach";
import { WHERE_TO_CHANGE } from "../ui/link-claim";
import type { FleetContext } from "./mock-fleet";
import type { CommandResult, DataSource } from "./source";

const OK: CommandResult = { status: "ok", data: null };

const NOT_HELD: CommandResult = {
	status: "error",
	error: { kind: "failed", message: "This link isn't waiting for an answer about its device" },
};

/** A made-up fingerprint for a sample link, the same each time. */
const SAMPLE_KEY = (link: LinkId) =>
	`ED25519 SHA256:${btoa(`sample-${link}`).replace(/=+$/, "").padEnd(43, "A").slice(0, 43)}`;

const addresses = (link: DeviceLink) =>
	linkRoutes(link).flatMap((r) => (r.route.type === "ssh" ? [r.route.host] : []));

/** The link claim calls over the sample entities. */
export const mockClaims = (
	context: FleetContext
): Pick<
	DataSource,
	"answerLinkClaim" | "askLinkClaimAgain" | "forgetLinkIdentity" | "linkClaimDetails"
> => {
	const { now, change } = context;
	const linkOf = (id: LinkId): DeviceLink | undefined => {
		for (const e of context.entities()) {
			if (e.entity === "device_link" && e.id === id) return e;
		}
		return undefined;
	};
	const edit = (id: LinkId, next: (link: DeviceLink) => DeviceLink) =>
		change((base) =>
			base.map((e) => (e.entity === "device_link" && e.id === id ? { ...e, ...next(e) } : e))
		);
	return {
		answerLinkClaim: async (id, same) => {
			const claim = linkOf(id)?.claim;
			if (!claim) return NOT_HELD;
			edit(id, (link) =>
				same
					? { ...link, device: claim.device, claim: null }
					: { ...link, claim: { ...claim, state: "different" } }
			);
			return OK;
		},
		askLinkClaimAgain: async (id) => {
			const claim = linkOf(id)?.claim;
			if (claim?.state !== "different") return NOT_HELD;
			edit(id, (link) => ({ ...link, claim: { ...claim, state: "asking" } }));
			return OK;
		},
		forgetLinkIdentity: async (id) => {
			const found = linkOf(id);
			if (!found)
				return { status: "error", error: { kind: "not_found", what: `link ${id}` } };
			if (found.route.type === "local") {
				return {
					status: "error",
					error: { kind: "failed", message: "This Mac always reaches its own maxd" },
				};
			}
			const device = found.device ?? found.claim?.device ?? null;
			let other: DeviceLink | undefined;
			for (const e of context.entities()) {
				if (e.entity === "device_link" && e.id !== id && e.device === device && !e.claim)
					other = e;
			}
			edit(id, (link) =>
				device !== null && other
					? {
							...link,
							device: null,
							claim: { device, known_by: other.id, state: "asking", since: now() },
						}
					: { ...link, device, claim: null }
			);
			return OK;
		},
		linkClaimDetails: async (id) => {
			const found = linkOf(id);
			const claim = found?.claim;
			if (!found || !claim) return NOT_HELD;
			const other = claim.known_by === null ? undefined : linkOf(claim.known_by);
			const details: LinkClaimDetails = {
				link: {
					link: found.id,
					name: found.name,
					addresses: addresses(found),
					fingerprints: [SAMPLE_KEY(found.id)],
					first_seen: claim.since,
				},
				known_by: other
					? {
							link: other.id,
							name: other.name,
							addresses: addresses(other),
							fingerprints: [SAMPLE_KEY(other.id)],
							first_seen: null,
						}
					: null,
				device_keys: other ? [SAMPLE_KEY(other.id)] : [],
				where_to_change: WHERE_TO_CHANGE,
			};
			return { status: "ok", data: details };
		},
	};
};
