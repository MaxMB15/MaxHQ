// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The time a render describes. Components read it through `useNow` rather
// than calling `Date.now()`, so a render stays pure and a test can pin the
// clock with `FixedNow`.

import { createContext, type ReactNode, useContext, useSyncExternalStore } from "react";
import type { Timestamp } from "../bindings";

type Clock = {
	now: Timestamp;
	readonly listeners: Set<() => void>;
	timer?: ReturnType<typeof setInterval>;
};

// One shared timer per resolution, running only while something listens.
const clocks = new Map<number, Clock>();

const clockFor = (every: number): Clock => {
	let clock = clocks.get(every);
	if (!clock) {
		clock = { now: Date.now(), listeners: new Set() };
		clocks.set(every, clock);
	}
	return clock;
};

const subscribers = new Map<number, (onChange: () => void) => () => void>();

const subscribeTo = (every: number) => {
	let subscribe = subscribers.get(every);
	if (!subscribe) {
		subscribe = (onChange) => {
			const clock = clockFor(every);
			clock.listeners.add(onChange);
			if (clock.timer === undefined) {
				clock.now = Date.now();
				clock.timer = setInterval(() => {
					clock.now = Date.now();
					for (const listener of clock.listeners) listener();
				}, every);
			}
			return () => {
				clock.listeners.delete(onChange);
				if (clock.listeners.size === 0 && clock.timer !== undefined) {
					clearInterval(clock.timer);
					clock.timer = undefined;
				}
			};
		};
		subscribers.set(every, subscribe);
	}
	return subscribe;
};

const FixedNowContext = createContext<Timestamp | null>(null);

/** Pins the clock for everything inside, for tests. */
export const FixedNow = ({ value, children }: { value: Timestamp; children: ReactNode }) => (
	<FixedNowContext value={value}>{children}</FixedNowContext>
);

/** The current time, updated every `every` milliseconds. */
export const useNow = (every = 1000): Timestamp => {
	const fixed = useContext(FixedNowContext);
	const live = useSyncExternalStore(subscribeTo(every), () => clockFor(every).now);
	return fixed ?? live;
};
