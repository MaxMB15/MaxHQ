// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PictureSet, useInPictureSet } from "./picture-set";

// A picture that joins the set around it, as the timeline's pictures do.
const Shot = ({ name }: { name: string }) => {
	const { place, open: openInSet } = useInPictureSet({
		name,
		picture: { kind: "url", url: `data:image/png;${name}` },
	});
	return (
		<button ref={place} type="button" onClick={openInSet ?? undefined}>
			{name}
		</button>
	);
};

const showSet = (names: string[]) =>
	render(
		<>
			<PictureSet>
				{names.map((name) => (
					<Shot key={name} name={name} />
				))}
			</PictureSet>
			<textarea aria-label="Composer" />
		</>
	);

const caption = () => screen.getByRole("dialog").textContent;

describe("a set of pictures", () => {
	it("opens on the one clicked and steps left and right, stopping at the ends", () => {
		showSet(["one.png", "two.png", "three.png"]);
		fireEvent.click(screen.getByRole("button", { name: "two.png" }));
		expect(caption()).toContain("2 / 3");
		expect(caption()).toContain("two.png");

		fireEvent.click(screen.getByRole("button", { name: "Next picture" }));
		expect(caption()).toContain("3 / 3three.png");
		expect(screen.getByRole("button", { name: "Next picture" })).toHaveProperty(
			"disabled",
			true
		);

		fireEvent.keyDown(document.activeElement ?? document.body, { key: "ArrowLeft" });
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "ArrowLeft" });
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "ArrowLeft" });
		expect(caption()).toContain("1 / 3one.png");
		expect(screen.getByRole("button", { name: "Previous picture" })).toHaveProperty(
			"disabled",
			true
		);
	});

	it("keeps the arrow keys from the composer and the timeline", () => {
		showSet(["one.png", "two.png"]);
		const heard = vi.fn();
		const composer = screen.getByRole("textbox", { name: "Composer" });
		composer.addEventListener("keydown", heard);
		window.addEventListener("keydown", heard);
		fireEvent.click(screen.getByRole("button", { name: "one.png" }));
		fireEvent.keyDown(composer, { key: "ArrowRight" });
		expect(caption()).toContain("2 / 2");
		expect(heard).not.toHaveBeenCalled();
		window.removeEventListener("keydown", heard);
	});

	it("shows a picture of its own without arrows or a count", () => {
		showSet(["only.png"]);
		fireEvent.click(screen.getByRole("button", { name: "only.png" }));
		expect(caption()).toBe("only.png");
		expect(screen.queryByRole("button", { name: "Next picture" })).toBeNull();
	});

	it("gives focus back and leaves the arrow keys alone once closed", async () => {
		showSet(["one.png", "two.png"]);
		const heard = vi.fn();
		window.addEventListener("keydown", heard);
		const one = screen.getByRole("button", { name: "one.png" });
		one.focus();
		fireEvent.click(one);
		fireEvent.click(screen.getByRole("button", { name: "Close" }));
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
		await waitFor(() => expect(document.activeElement).toBe(one));
		fireEvent.keyDown(document.body, { key: "ArrowRight" });
		expect(heard).toHaveBeenCalledTimes(1);
		window.removeEventListener("keydown", heard);
	});
});
