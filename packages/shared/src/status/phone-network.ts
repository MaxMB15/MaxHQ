// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The phone's network as NetInfo reports it, read for the phone's core and
// the banner. The core keeps each link's last good route per network kind,
// and a new kind wakes every link. So it hears a kind only when it changes,
// and only while the app is in front, since a wake in the background would
// bring back the links the app closed. A change while away goes over once
// the app is back.

/** The network the phone is on, as the phone's core takes it. */
export type PhoneNetwork = "wifi" | "cellular" | "none";

/** The parts of NetInfo's state read here. */
export type NetState = { readonly type: string; readonly isConnected: boolean | null };

/** The kind for the core, or null while the phone can't tell. Ethernet counts as Wi-Fi. */
export const phoneNetworkOf = (state: NetState): PhoneNetwork | null => {
	if (state.isConnected === false || state.type === "none") return "none";
	if (state.type === "cellular") return "cellular";
	if (state.type === "wifi" || state.type === "ethernet") return "wifi";
	return null;
};

/** Whether the phone is online, as the banner takes it: false only once it knows. */
export const phoneOnline = (state: NetState): boolean => state.isConnected !== false;

/** Tells the core each new network kind while the app is in front. */
export const createNetworkTeller = (tell: (kind: PhoneNetwork) => void, active: boolean) => {
	let told: PhoneNetwork | null = null;
	let latest: PhoneNetwork | null = null;
	const flush = () => {
		if (!active || latest === null || latest === told) return;
		told = latest;
		tell(latest);
	};
	return {
		/** Takes what NetInfo says now. */
		network: (state: NetState) => {
			latest = phoneNetworkOf(state);
			flush();
		},
		/** Takes whether the app is in front. */
		active: (now: boolean) => {
			active = now;
			flush();
		},
	};
};
