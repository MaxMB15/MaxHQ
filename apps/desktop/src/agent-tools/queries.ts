// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent tools' reads, shared by every screen that shows them: every
// device's permissions and access keys, and the defaults a new permission
// starts with. A change asks for them all again.

import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useServices } from "@/app/services";

const PERMISSIONS = "permissions";
const KEYS = "access-keys";
const DEFAULTS = "agent-defaults";

/** Every connected device's permissions, each once. */
export const usePermissions = () => {
	const { source } = useServices();
	return useQuery({ queryKey: [PERMISSIONS], queryFn: () => source.listPermissions(null) });
};

/** Every connected device's access keys. */
export const useKeys = () => {
	const { source } = useServices();
	return useQuery({ queryKey: [KEYS], queryFn: () => source.listKeys(null) });
};

/** The limits and depth a new permission starts with. */
export const useAgentDefaults = () => {
	const { source } = useServices();
	return useQuery({ queryKey: [DEFAULTS], queryFn: () => source.agentDefaults() });
};

/** Asks for the permissions, keys and defaults again after a change. */
export const useRefreshAgentTools = () => {
	const queries = useQueryClient();
	return () =>
		Promise.all(
			[PERMISSIONS, KEYS, DEFAULTS].map((key) =>
				queries.invalidateQueries({ queryKey: [key] })
			)
		);
};
