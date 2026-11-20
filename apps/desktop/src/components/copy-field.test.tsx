// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ClippedValue, CopyField, InlineCopy } from "./copy-field";

const PATH = "/home/dev/.config/maxhq/keys/devbox-github-app.private-key.pem";

describe("copyable values", () => {
	it("select on their own, with the full value on hover and a copy icon", () => {
		render(
			<>
				<CopyField value="ssh-ed25519 AAAAC3Nza" label="Public key" />
				<InlineCopy text="claude setup-token" />
				<ClippedValue value={PATH} label="Copy path" />
			</>
		);
		const key = screen.getByLabelText("Public key");
		expect(key).toHaveClass("selectable");
		expect(key).toHaveAttribute("title", "ssh-ed25519 AAAAC3Nza");
		expect(screen.getByText("claude setup-token")).toHaveClass("selectable");

		const path = screen.getByText(PATH);
		expect(path).toHaveClass("selectable", "truncate");
		expect(path).toHaveAttribute("title", PATH);
		expect(screen.getByRole("button", { name: "Copy path" })).not.toHaveClass("selectable");
	});
});
