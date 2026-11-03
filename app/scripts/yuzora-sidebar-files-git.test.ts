import { expect, test } from "bun:test";
import { readGit } from "../defaults/mods/yuzora-sidebar-files/git-state.js";

const status = { top: "/r", branch: "main", marks: {} };

test("a folder that is not a repo is not a failure", async () => {
  expect(await readGit(async () => null, "/x", {})).toEqual({ git: null, failed: null });
});

test("a repo that was read is passed on", async () => {
  expect(await readGit(async () => status, "/r", {})).toEqual({ git: status, failed: null });
});

test("a git that could not be read is told apart from not a repo", async () => {
  const out = await readGit(async () => { throw new Error("git timed out"); }, "/r", {});
  expect(out.git).toBeNull();
  expect(out.failed).toBe("git timed out");
  expect((await readGit(() => { throw "boom"; }, "/r", {})).failed).toBe("boom");
});
