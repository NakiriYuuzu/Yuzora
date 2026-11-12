// After a pull, merge, cherry-pick or rebase stops on conflicts, the Git panes say how many files have them.
// Run: bun test scripts/yuzora-git-counts.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

for (const mod of ["yuzora-sidebar-git", "yuzora-git-log", "yuzora-git-rebase", "yuzora-git-repo"]) {
  test(`${mod} says one file has conflicts in the singular`, async () => {
    const source = await readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8");
    const helper = /^const conflictsText = .*;$/m.exec(source)![0];
    const said = new Function("t", `${helper}\nreturn conflictsText;`)((key: string, values?: object) => [key, values ?? null]);
    expect([1, 2, 10].map(said)).toEqual([["conflicts_one", null], ["conflicts_other", { count: 2 }], ["conflicts_other", { count: 10 }]]);
    expect(source).toContain("conflictsText(out.conflicts.length)");
    expect(source).not.toMatch(/t\("conflicts", \{/);
    const strings = JSON.parse(await readFile(new URL(`../defaults/mods/${mod}/i18n/en.json`, import.meta.url), "utf8"));
    expect(strings.conflicts_one).toBe("1 file has conflicts.");
    expect(strings.conflicts_other).toBe("{count} files have conflicts.");
  });
}
