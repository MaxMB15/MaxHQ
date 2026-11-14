// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A chip on an agent's header for each test machine it holds, with its
// kind and time left, opening the lease's view.

import { MonitorSmartphone } from "lucide-react";
import { useState } from "react";

import { useView } from "@/app/services";
import type { AgentId, LeaseId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { leaseChip, leaseLine } from "@maxhq/shared/test-hosts/format";
import { leasesOf } from "@maxhq/shared/test-hosts/hosts";

import { LeaseView } from "./lease-view";

export const LeaseChips = ({ agent }: { agent: AgentId }) => {
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const [open, setOpen] = useState<LeaseId | null>(null);
	const leases = leasesOf(view, { kind: "agent", agent });
	// The view stays while open, to say a lease it shows has ended.
	if (leases.length === 0 && open === null) return null;
	return (
		<>
			{leases.map((lease) => (
				<button
					key={lease.id}
					type="button"
					title={leaseLine(lease, now)}
					onClick={() => setOpen(lease.id)}
					className="inline-flex h-[22px] shrink-0 items-center gap-[5px] rounded-md border border-track bg-secondary px-[7px] text-[11.5px] whitespace-nowrap text-secondary-foreground hover:bg-accent"
				>
					<MonitorSmartphone aria-hidden size={12} strokeWidth={2} />
					{leaseChip(lease, now)}
				</button>
			))}
			<LeaseView lease={open} onClose={() => setOpen(null)} />
		</>
	);
};
