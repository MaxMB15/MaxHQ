// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The globals this package uses that browsers and React Native both have. The
// tsconfig leaves out the DOM, so any other DOM global fails the typecheck.

declare function setTimeout(handler: () => void, timeout?: number): number;
declare function clearTimeout(id: number | undefined): void;
declare function setInterval(handler: () => void, timeout?: number): number;
declare function clearInterval(id: number | undefined): void;
declare function requestAnimationFrame(callback: (time: number) => void): number;
declare function btoa(data: string): string;
// Shaped as the DOM's, so the two declarations merge where both are seen.
interface Console {
	warn(...data: unknown[]): void;
}
// eslint-disable-next-line no-var -- a const would clash with the DOM's var.
declare var console: Console;
