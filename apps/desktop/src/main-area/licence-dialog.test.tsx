// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MAC } from "@maxhq/shared/data/mock-view";

import { renderSection, type SectionProps } from "./device-testing";
import { LicenceDialog } from "./licence-dialog";

const Open = ({ device, view }: SectionProps) => (
	<LicenceDialog device={device} view={view} open onClose={() => {}} />
);

describe("the licence dialog", () => {
	it("shows each licence in full and accepts it as read", async () => {
		const { source } = renderSection(Open, MAC, "Mac");
		const accept = vi.spyOn(source, "acceptLicence");
		const dialog = within(await screen.findByRole("dialog", { name: "Licences to accept" }));
		expect(
			await dialog.findByRole("region", { name: "android-sdk-license" })
		).toHaveTextContent("for Android SDK");
		expect(dialog.getByLabelText("android-sdk-license text")).toHaveTextContent(
			"Terms and conditions for the Android Software Development Kit."
		);
		fireEvent.click(dialog.getByRole("button", { name: "Accept android-sdk-license" }));
		expect(accept).toHaveBeenCalledWith(
			MAC,
			"android-sdk-license",
			"0d6f4b8e2c1a9f7d5b3e1c0a8f6d4b2e0c9a7f5d3b1e9c7a5f3d1b0e8c6a4f2d"
		);
		await waitFor(() =>
			expect(dialog.getByText("Nothing waits on a licence.")).toBeInTheDocument()
		);
	});
});
