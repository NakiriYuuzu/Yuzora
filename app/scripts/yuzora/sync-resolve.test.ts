import { describe, expect, test } from "bun:test";
import { resolve, rules } from "./sync-resolve";

const conflict = (ours: string, theirs: string) => `<<<<<<< HEAD\n${ours}\n=======\n${theirs}\n>>>>>>> upstream`;

describe("the conflicts every upstream release brings", () => {
  test("Cargo.toml keeps the workspace version, and nothing else of Yuzora's by default", () => {
    const text = `[package]\nname = "herdr-app"\n${conflict("version.workspace = true", 'version = "0.1.16"')}\nedition.workspace = true\n`;
    expect(resolve(text, rules["Cargo.toml"]!)).toEqual({
      text: '[package]\nname = "herdr-app"\nversion.workspace = true\nedition.workspace = true\n',
      left: 0,
    });
    // A dependency both sides changed is a person's call.
    const dependency = conflict('gpui = "0.3.8"', 'gpui = "0.4.0"');
    expect(resolve(dependency, rules["Cargo.toml"]!)).toEqual({ text: dependency, left: 1 });
    const both = conflict('version.workspace = true\ngpui = "0.3.8"', 'version = "0.1.16"\ngpui = "0.4.0"');
    expect(resolve(both, rules["Cargo.toml"]!).left).toBe(1);
  });

  test("CHANGELOG.md puts Yuzora's entries first and upstream's new ones after them", () => {
    const ours = "<!-- yuzora: Yuzora's entries sit above Herdr's -->\n## [0.1.2] - 2026-11-13\n\n- Ours.";
    const theirs = "## [0.1.16] - 2026-11-14\n\n- Theirs.";
    const text = `# Herdr changelog\n\n${conflict(ours, theirs)}\n\n## [0.1.15] - 2026-11-10\n`;
    const result = resolve(text, rules["CHANGELOG.md"]!);
    expect(result).toEqual({ text: `# Herdr changelog\n\n${ours}\n\n${theirs}\n\n## [0.1.15] - 2026-11-10\n`, left: 0 });
    expect(result.text.match(/^## \[[^\]]+\]/gm)).toEqual(["## [0.1.2]", "## [0.1.16]", "## [0.1.15]"]);
    // Two edits of one entry are not two new entries.
    const edit = conflict("- A line Yuzora changed.", "- The line as upstream changed it.");
    expect(resolve(edit, rules["CHANGELOG.md"]!)).toEqual({ text: edit, left: 1 });
  });

  test("Cargo.lock and the notices take Yuzora's side: both are written again from the merged manifest", () => {
    const lock = `name = "herdr-app"\n${conflict('version = "0.1.2"', 'version = "0.1.16"')}\ndependencies = [\n`;
    expect(resolve(lock, rules["Cargo.lock"]!)).toEqual({ text: 'name = "herdr-app"\nversion = "0.1.2"\ndependencies = [\n', left: 0 });
    const notices = `${conflict("  herdr-app 0.1.2\n  herdr-app 0.1.2", "  herdr-app 0.1.15")}\n\n${conflict("a", "b")}\n`;
    expect(resolve(notices, rules["THIRD-PARTY-NOTICES"]!)).toEqual({ text: "  herdr-app 0.1.2\n  herdr-app 0.1.2\n\na\n", left: 0 });
  });
});

describe("resolve", () => {
  test("text without a conflict comes back as it was", () => {
    const text = "a\n=======\nb\n";
    expect(resolve(text, () => [])).toEqual({ text, left: 0 });
  });

  test("the base of a diff3 conflict belongs to neither side", () => {
    const text = "<<<<<<< HEAD\nours\n||||||| base\nwas\n=======\ntheirs\n>>>>>>> upstream\nafter";
    const seen: unknown[] = [];
    const result = resolve(text, c => (seen.push(c), c.theirs));
    expect(seen).toEqual([{ ours: ["ours"], theirs: ["theirs"] }]);
    expect(result).toEqual({ text: "theirs\nafter", left: 0 });
  });

  test("a conflict the rule leaves keeps its markers, and one that never closes counts", () => {
    const one = conflict("a", "b");
    expect(resolve(`${one}\n${conflict("c", "d")}`, c => (c.ours[0] === "c" ? ["c"] : null))).toEqual({ text: `${one}\nc`, left: 1 });
    expect(resolve("<<<<<<< HEAD\na\n=======\nb", c => c.ours)).toEqual({ text: "<<<<<<< HEAD\na\n=======\nb", left: 1 });
  });
});
