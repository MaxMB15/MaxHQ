// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { pieceMark } from "@maxhq/shared/pills/sent";

import { Markdown } from "./markdown";
import { loadMath, mathHtml } from "./math";

const hooks = vi.hoisted(() => ({
	draw: vi.fn<(source: string) => Promise<unknown>>(),
}));

// Mermaid needs a real page to measure text. WebKit runs it against the app's
// own rules, and these tests cover what the feed does with what it draws.
vi.mock("./diagram-render", () => {
	const drawn = new Map<string, unknown>();
	return {
		drawnDiagram: (source: string) => drawn.get(source),
		drawDiagram: async (source: string) => {
			const result = await hooks.draw(source);
			drawn.set(source, result);
			return result;
		},
	};
});

const show = (text: string, props: Partial<Parameters<typeof Markdown>[0]> = {}) =>
	render(
		<ServicesProvider source={mockSource({ tick: false })}>
			<Markdown text={text} {...props} />
		</ServicesProvider>
	);

const picture = (width: number, height: number) => ({
	state: "drawn",
	url: "data:image/svg+xml,x",
	width,
	height,
	whole: () => "data:image/svg+xml,whole",
});

const fence = (name: string, body: string) => `\`\`\`${name}\n${body}\n\`\`\``;

describe("a Mermaid fence", () => {
	it("shows a picture of its size, and its source on asking", async () => {
		hooks.draw.mockResolvedValue(picture(400, 200));
		show(fence("mermaid", "graph TD; A-->One"));
		const image = await screen.findByAltText("graph TD; A-->One");
		expect(image.style.aspectRatio).toBe("400 / 200");
		expect(image.closest("[data-find-skip]")).not.toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Source" }));
		expect(screen.getByText("graph TD; A-->One").tagName).toBe("CODE");
		fireEvent.click(screen.getByRole("button", { name: "Diagram" }));
		expect(screen.getByAltText("graph TD; A-->One")).toBeTruthy();
	});

	it("keeps a tall one short enough to read in the feed", async () => {
		hooks.draw.mockResolvedValue(picture(400, 1600));
		show(fence("mermaid", "graph TD; A-->Tall"));
		const image = await screen.findByAltText("graph TD; A-->Tall");
		expect(image.style.maxWidth).toBe("140px");
	});

	it("shows its source while it streams, and draws nothing", () => {
		hooks.draw.mockClear();
		show(fence("mermaid", "graph TD; A-->Live"), { streaming: true });
		expect(screen.getByText(/A-->Live/)).toBeTruthy();
		expect(hooks.draw).not.toHaveBeenCalled();
	});

	it("shows its source and why when it can't be drawn", async () => {
		hooks.draw.mockResolvedValue({ state: "failed", reason: "Parse error on line 1" });
		show(fence("mermaid", "not a diagram"));
		expect(await screen.findByText(/Parse error on line 1/)).toBeTruthy();
		expect(screen.getByText(/not a diagram/)).toBeTruthy();
	});

	it("is the same whatever case names the fence", async () => {
		hooks.draw.mockResolvedValue(picture(100, 100));
		show(fence("Mermaid", "graph TD; A-->Named"));
		expect(await screen.findByAltText("graph TD; A-->Named")).toBeTruthy();
	});

	it("leaves any other fence as code", () => {
		const { container } = show(fence("ts", "const one = 1;"));
		expect(container.textContent).toContain("const one = 1;");
		expect(container.querySelector("img")).toBeNull();
	});
});

describe("math", () => {
	it("draws a formula in a sentence, and leaves it out of find", async () => {
		const { container } = show("Mass $E = mc^2$ here");
		await waitFor(() => expect(container.querySelector(".katex")).not.toBeNull());
		const shown = screen.getByRole("img", { name: "E = mc^2" });
		expect(shown.hasAttribute("data-find-skip")).toBe(true);
		expect(shown.firstElementChild?.getAttribute("aria-hidden")).toBe("true");
		expect(container.querySelector(".katex-display")).toBeNull();
	});

	it.each([
		["two lines of $$", "$$\nx^2\n\ny\n$$"],
		["a math fence", fence("math", "x^2")],
	])("draws %s on its own", async (_, text) => {
		const { container } = show(text);
		await waitFor(() => expect(container.querySelector(".katex-display")).not.toBeNull());
	});

	it.each(["From $5-$10 now", "It costs $5 and then $6", "a$b$c", "Set PATH=$HOME and $USER"])(
		"leaves the dollars in %s as they are",
		(text) => {
			const { container } = show(text);
			expect(container.textContent).toBe(text);
			expect(container.querySelector("[role=img]")).toBeNull();
		}
	);

	it("takes no link or script from a formula", async () => {
		const { container } = show(
			"$\\href{javascript:alert(1)}{x}$ and $\\includegraphics{a.png}$"
		);
		await waitFor(() => expect(container.querySelector(".katex")).not.toBeNull());
		expect(container.querySelector("a")).toBeNull();
		expect(container.querySelector("img")).toBeNull();
	});
});

describe("a callout", () => {
	it("shows the name of its kind over its text", () => {
		const { container } = show("> [!WARNING]\n> Mind the gap");
		const quote = container.querySelector("blockquote")!;
		expect(quote.className).toContain("border-status-busy");
		expect(within(quote).getByText("Warning").hasAttribute("data-find-skip")).toBe(true);
		expect(within(quote).getByText("Mind the gap")).toBeTruthy();
		expect(quote.textContent).not.toContain("[!");
	});

	it("shows its own title, which find can reach", () => {
		const { container } = show("> [!tip]- Try this\n> and that");
		const title = within(container).getByText("Try this");
		expect(title.closest("[data-find-skip]")).toBeNull();
		expect(container.textContent).not.toContain("Tip");
		expect(container.textContent).toContain("and that");
	});

	it("reads Obsidian's other names", () => {
		const { container } = show("> [!FAQ]\n> Why");
		expect(within(container).getByText("Question")).toBeTruthy();
	});

	it("shows a quote with no text of its own", () => {
		const { container } = show("> [!NOTE]");
		expect(within(container).getByText("Note")).toBeTruthy();
	});

	it("leaves a word it doesn't know as a quote", () => {
		const { container } = show("> [!NOPE]\n> Plain");
		expect(container.querySelector("blockquote")!.textContent).toContain("[!NOPE]");
	});
});

describe("footnotes and references", () => {
	const NOTED =
		"One[^a] and [a link][site].\n\nTwo\n\n[^a]: The note\n\n[site]: https://example.com";

	it("find their definitions in another block", () => {
		const { container } = show(NOTED);
		expect(container.querySelector("a[href='https://example.com']")).not.toBeNull();
		expect(container.querySelector("section")!.textContent).toContain("The note");
	});

	it("link within their own message, wherever two have the same names", () => {
		const { container } = render(
			<ServicesProvider source={mockSource({ tick: false })}>
				<Markdown text={NOTED} />
				<Markdown text={NOTED} />
			</ServicesProvider>
		);
		const notes = [...container.querySelectorAll("section li")].map((note) => note.id);
		expect(notes).toHaveLength(2);
		expect(new Set(notes).size).toBe(2);
		const refs = container.querySelectorAll<HTMLAnchorElement>("sup a");
		expect(refs[1]!.getAttribute("href")).toBe(`#${notes[1]}`);
		expect(refs[1]!.getAttribute("target")).toBeNull();
	});

	it("moves to the note on a click", () => {
		const { container } = show(NOTED);
		const note = container.querySelector<HTMLElement>("section li")!;
		note.scrollIntoView = vi.fn();
		fireEvent.click(container.querySelector("sup a")!);
		expect(note.scrollIntoView).toHaveBeenCalled();
	});

	it("marks the way back as something find leaves out", () => {
		const { container } = show(NOTED);
		expect(container.querySelector("[data-footnote-backref]")).not.toBeNull();
		expect(container.querySelector("section h2")!.className).toContain("sr-only");
	});
});

describe("raw HTML in a reply", () => {
	it("shows what the allowlist keeps", () => {
		const { container } = show(
			"<details open>\n<summary>More</summary>\n\nInside <kbd>Ctrl</kbd> and <mark>this</mark>\n\n</details>"
		);
		expect(container.querySelector("details")!.hasAttribute("open")).toBe(true);
		expect(screen.getByText("More").tagName).toBe("SUMMARY");
		expect(screen.getByText("Ctrl").tagName).toBe("KBD");
		expect(screen.getByText("this").tagName).toBe("MARK");
	});

	it.each([
		["a script", "<script>alert(1)</script>Seen", "alert(1)"],
		["a style", "<style>body{display:none}</style>Seen", "display:none"],
		["a frame", "<iframe src='https://example.com'>inner</iframe>Seen", "inner"],
		["an object", "<object data='x'></object>Seen", "<object"],
		[
			"a form",
			"<form action='https://example.com'><button>Send</button></form>Seen",
			"<button>",
		],
	])("shows %s as text, where it can do nothing", (_, text, words) => {
		const { container } = show(text);
		expect(container.textContent).toContain("Seen");
		expect(container.textContent).toContain(words);
		for (const tag of ["script", "style", "iframe", "object", "form", "button"])
			expect(container.querySelector(tag)).toBeNull();
	});

	it("takes the handlers, styles and ids off what it keeps", () => {
		const { container } = show(
			'<div onclick="alert(1)" style="position:fixed" id="root" class="fixed">Seen</div>'
		);
		const div = screen.getByText("Seen");
		for (const name of ["onclick", "style", "id", "class"])
			expect(div.hasAttribute(name)).toBe(false);
		expect(container.querySelector("#root")).toBeNull();
	});

	it("leaves out a link that would run code", () => {
		const { container } = show(
			'<a href="javascript:alert(1)">Click</a> and <a href="https://example.com">Go</a>'
		);
		expect(screen.getByText("Click").getAttribute("href")).toBeNull();
		const go = container.querySelector<HTMLAnchorElement>("a[href='https://example.com']")!;
		expect(go.target).toBe("_blank");
		expect(go.rel).toBe("noreferrer");
	});

	it("shows a message you wrote as you wrote it", () => {
		const { container } = show("Keep <b>this</b> as typed", {
			written: { draw: () => null, plain: () => "" },
		});
		expect(container.textContent).toBe("Keep <b>this</b> as typed");
		expect(container.querySelector("b")).toBeNull();
	});
});

describe("text that could break the feed", () => {
	it.each([
		["a word every object has", "> [!constructor] Title\n> Body", "[!constructor]"],
		["another of them", "> [!toString]\n> Body", "[!toString]"],
	])("reads a callout's %s as a plain quote", (_, text, shown) => {
		const { container } = show(text);
		expect(container.textContent).toContain(shown);
		expect(container.querySelector("blockquote")).not.toBeNull();
	});

	it.each(["constructor", "__proto__", "toString", "valueOf"])(
		"shows a callout title that names %s",
		(word) => {
			const { container } = show(`<div data-alert-title="${word}">Seen</div>`);
			expect(container.textContent).toContain("Seen");
		}
	);

	it.each([
		["in a sentence", (deep: string) => `$${deep}$`],
		["on its own lines", (deep: string) => fence("math", deep)],
	])("shows the source of a formula too deep to read, %s", async (_, wrap) => {
		const deep = `${"{".repeat(20000)}x${"}".repeat(20000)}`;
		await loadMath();
		expect(mathHtml(deep, false)).toBeNull();
		const { container } = show(wrap(deep));
		expect(container.textContent).toContain("{{{x}}}");
	});

	it("caps the sizes a formula names", async () => {
		await loadMath();
		expect(mathHtml("\\rule{99999em}{99999em}", false)).not.toContain("99999");
	});

	it("shows a block's text when it can't draw, and tries its next text", () => {
		const breaking = {
			draw: () => {
				throw new Error("broken");
			},
			plain: () => "",
		};
		const error = vi.spyOn(console, "error").mockImplementation(() => {});
		const shown = (text: string) => (
			<ServicesProvider source={mockSource({ tick: false })}>
				<Markdown text={text} written={breaking} />
			</ServicesProvider>
		);
		const { container, rerender } = render(shown(`Before ${pieceMark(0)} after`));
		expect(container.textContent).toContain("Before");
		expect(container.textContent).toContain("after");
		rerender(shown("Nothing to draw"));
		expect(container.textContent).toBe("Nothing to draw");
		error.mockRestore();
	});

	it("gets through more quote marks than a parser can follow", () => {
		const { container } = show(`${">".repeat(20000)} deep`);
		expect(container.textContent).toContain("deep");
	});
});

describe("a tag in a reply that means nothing to it", () => {
	it.each([
		["Add a <style> block to the page, then run the tests."],
		["Set the <title> of the page, then deploy to prod."],
		["Use Vec<String> in <Composer> here."],
		["Keep <kbd>open"],
		["Close </kbd> here"],
		["A <!DOCTYPE html> line"],
	])("reads as written: %s", (text) => {
		const { container } = show(text);
		expect(container.textContent).toBe(text);
	});

	it("keeps the Markdown around it", () => {
		const { container } = show("Put the note in a <textarea> and keep **this** too.");
		expect(container.textContent).toBe("Put the note in a <textarea> and keep this too.");
		expect(screen.getByText("this").tagName).toBe("STRONG");
	});

	it("keeps the blocks after it", () => {
		const { container } = show("Use a <select> here\n\n- one\n- two");
		expect(container.querySelectorAll("li")).toHaveLength(2);
		expect(container.textContent).toContain("<select>");
	});

	it("still leaves a comment out", () => {
		const { container } = show("Before <!-- hidden --> after");
		expect(container.textContent).not.toContain("hidden");
	});

	it("still draws a tag with its pair, and those that need none", () => {
		const { container } = show("<ul><li>one<li>two</ul>\n\nLine<br>break and <b>this</b>");
		expect(container.querySelectorAll("li")).toHaveLength(2);
		expect(container.querySelector("br")).not.toBeNull();
		expect(screen.getByText("this").tagName).toBe("B");
	});

	it("reads a tag inside an attribute as part of that attribute", () => {
		const { container } = show('Then <kbd title="</kbd><b>">y</kbd> and <b>z</b>');
		expect(screen.getByText("y").tagName).toBe("KBD");
		expect(container.querySelectorAll("b")).toHaveLength(1);
	});
});

describe("raw HTML next to what Markdown draws", () => {
	it("keeps a picture from a data link the same with a tag beside it", () => {
		const url = "data:image/png;base64,AAAA";
		const [alone, beside] = [`![a](${url})`, `![a](${url}) <kbd>x</kbd>`].map((text) => {
			const { container } = show(text);
			return container.querySelector("[data-find-skip]")?.innerHTML;
		});
		expect(alone).toBeTruthy();
		expect(beside).toBe(alone);
	});

	it("still takes out a link that would run code, on a picture too", () => {
		const { container } = show('<img src="javascript:alert(1)" alt="a">');
		expect(container.innerHTML).not.toContain("javascript:");
	});

	it("shows a pre's text with no code inside", () => {
		const { container } = show("<pre>hello</pre>");
		expect(container.textContent).toContain("hello");
	});

	it("shows all of a pre's text around its tags", () => {
		const { container } = show("<pre>a <b>x</b></pre>");
		expect(container.textContent).toContain("a x");
	});
});

describe("a message you wrote", () => {
	const written = { draw: () => null, plain: () => "" };

	it("shows a block of several lines of code as a block", () => {
		const { container } = show(fence("ts", "const one = 1;\nconst two = 2;"), { written });
		expect(container.textContent).toContain("const one = 1;\nconst two = 2;");
		expect(container.textContent).not.toContain("```");
	});

	it("draws math and a callout as a reply does", async () => {
		const { container } = show("> [!NOTE]\n> Mind $x^2$", { written });
		await waitFor(() => expect(container.querySelector(".katex")).not.toBeNull());
		expect(within(container).getByText("Note")).toBeTruthy();
	});
});

describe("smaller things", () => {
	it("draws a task list without bullets", () => {
		const { container } = show("- [x] Done\n- [ ] Not yet");
		expect(container.querySelector("ul")!.className).toContain("list-none");
		expect(container.querySelectorAll("input")).toHaveLength(2);
	});

	it("draws every level of heading", () => {
		show("##### Five\n\n###### Six");
		expect(screen.getByText("Five").tagName).toBe("H6");
		expect(screen.getByText("Six").tagName).toBe("H6");
	});
});

describe("a visual", () => {
	const page = '<div id="chart">Sales</div>';
	const opened = `\`\`\`html visual title="Sales by month"\n${page}`;
	const visual = `${opened}\n\`\`\``;

	it("draws a closed html visual fence in a card", () => {
		const { container } = show(`Here:\n\n${visual}`);
		const card = container.querySelector("figure");
		expect(card).not.toBeNull();
		expect(within(card!).getByText("Sales by month")).toBeTruthy();
		expect(card!.querySelector("iframe")).not.toBeNull();
	});

	it("leaves a plain html fence as code", () => {
		const { container } = show(fence("html", page));
		expect(container.querySelector("figure")).toBeNull();
		expect(container.querySelector("iframe")).toBeNull();
		expect(container.textContent).toContain(page);
	});

	it("waits for its fence to close while it streams", () => {
		const { container } = show(opened, { streaming: true });
		expect(container.querySelector("iframe")).toBeNull();
		expect(screen.getByRole("status")).toHaveTextContent(
			"Drawing when the block ends, 27 B so far"
		);
	});

	it("draws once its fence closes, in the same message", () => {
		const { container, rerender } = show(opened, { streaming: true });
		rerender(
			<ServicesProvider source={mockSource({ tick: false })}>
				<Markdown text={visual} streaming />
			</ServicesProvider>
		);
		expect(container.querySelector("iframe")).not.toBeNull();
	});
});
