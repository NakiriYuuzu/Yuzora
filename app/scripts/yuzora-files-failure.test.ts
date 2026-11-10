// yuzora(v0.1.0 F1): what the Files sidebar tells a person whose new file, new folder, rename, trash or folder
// listing failed. The messages are the ones yuzora-files makes (fsops.rs, remote_entry.rs, docs.rs), with the
// prefix the engine puts before them, run through failure.js with the sidebar's real English strings.
// Run: bun test scripts/yuzora-files-failure.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const dir = new URL("../defaults/mods/yuzora-sidebar-files/", import.meta.url);
const [source, strings] = await Promise.all([readFile(new URL("failure.js", dir), "utf8"), readFile(new URL("i18n/en.json", dir), "utf8")]);
const en = JSON.parse(strings);
const t = (key: string, vars: Record<string, string> = {}) =>
  String(key.split(".").reduce((o: any, k) => o?.[k], en)).replace(/\{(\w+)\}/g, (_, k) => vars[k]);
const code = source.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export (?=function|const)/gm, "");
const { reason, LIMIT } = new Function("t", `${code}\nreturn { reason, LIMIT };`)(t) as {
  reason: (error: unknown, at?: { name?: string; machine?: string | null; kind?: string }) => string;
  LIMIT: number;
};

const host = (call: string, message: string) => new Error(`\`yuzora:files.${call}\`: ${message}`);
const GROUP = "a name is added to or removed from a folder only by someone who can write to it and enter it";

// The toast is 340 px wide with 18 px on both sides, a dot and its gap: 284 px of 12 px text, which is about 40
// characters of the interface font with some to spare. What is longer is cut by the toast, and the reason goes first.
test("the limit is what a toast shows of a line", () => {
  expect(LIMIT).toBe(40);
});

const cases: [string, Error | string | undefined, { name?: string; machine?: string | null; kind?: string }, string][] = [
  ["a name that is taken, here", host("createFile", "cannot create /tmp/run/notes.txt: File exists (os error 17)"), { name: "notes.txt", kind: "new-file" }, "notes.txt already exists"],
  ["a folder name that is taken, here", host("createFolder", "cannot create /tmp/run/src: File exists (os error 17)"), { name: "src", kind: "new-folder" }, "src already exists"],
  ["a rename onto a taken name", host("rename", "/tmp/run/b.txt already exists"), { name: "b.txt", kind: "rename" }, "b.txt already exists"],
  ["a taken name on Windows", host("createFile", "cannot create C:\\run\\n.txt: The file exists. (os error 80)"), { name: "n.txt", kind: "new-file" }, "n.txt already exists"],
  ["a taken name on a machine", host("createFile", "/srv/run/notes.txt already exists"), { name: "notes.txt", machine: "lab", kind: "new-file" }, "notes.txt already exists"],
  ["a folder that cannot be written, here", host("createFile", "cannot create /ro/n.txt: Permission denied (os error 13)"), { name: "n.txt", kind: "new-file" }, "The folder is not writable"],
  ["a folder that cannot be written, on Windows", host("rename", "cannot rename C:\\ro\\a.txt: Access is denied. (os error 5)"), { name: "b.txt", kind: "rename" }, "The folder is not writable"],
  ["a folder that cannot be written, on a machine", host("createFile", `cannot create /srv/ro/n.txt: you may not change /srv/ro on lab (${GROUP})`), { name: "n.txt", machine: "lab", kind: "new-file" }, "The folder is not writable"],
  ["a read-only disk", host("createFolder", "cannot create /Volumes/cd/n: Read-only file system (os error 30)"), { name: "n", kind: "new-folder" }, "The disk is read-only"],
  ["a full disk", host("createFile", "cannot create /tmp/n.txt: No space left on device (os error 28)"), { name: "n.txt", kind: "new-file" }, "The disk is full"],
  ["a name that is too long", host("createFile", "cannot create /tmp/long: File name too long (os error 63)"), { name: "long", kind: "new-file" }, "That name is too long"],
  ["a folder that is gone, here", host("createFile", "cannot create /gone/n.txt: No such file or directory (os error 2)"), { name: "n.txt", kind: "new-file" }, "The folder no longer exists"],
  ["a folder that is gone, on Windows", host("createFile", "cannot create C:\\gone\\n.txt: The system cannot find the path specified. (os error 3)"), { name: "n.txt", kind: "new-file" }, "The folder no longer exists"],
  ["a folder that is gone, on a machine", host("createFile", "cannot create /srv/gone/n.txt: the folder /srv/gone does not exist on lab"), { name: "n.txt", machine: "lab", kind: "new-file" }, "The folder no longer exists"],
  ["a part of the path that is a file", host("createFile", "cannot create /tmp/f.txt/n: Not a directory (os error 20)"), { name: "n", kind: "new-file" }, "A part of the path is not a folder"],
  ["an item renamed that is gone, here", host("rename", "cannot rename /tmp/a.txt: No such file or directory (os error 2)"), { name: "b.txt", kind: "rename" }, "That item is no longer there"],
  ["an item renamed that is gone, on a machine", host("rename", "cannot rename /srv/a.txt: it is no longer there on lab"), { name: "b.txt", machine: "lab", kind: "rename" }, "That item is no longer there"],
  ["a machine that is not connected", host("createFile", "cannot create /srv/n.txt on lab: machine lab is not connected"), { name: "n.txt", machine: "lab", kind: "new-file" }, "Can't reach lab"],
  ["a machine that does not answer in time", host("rename", "cannot rename /srv/a.txt on lab: took longer than 30 s on lab and was stopped. It may or may not have been done: reload the folder to see"), { name: "b.txt", machine: "lab", kind: "rename" }, "lab took too long: check again"],
  ["an answer that could not be read", host("createFile", "cannot create /srv/n.txt on lab: the answer could not be read; it may or may not have been done"), { name: "n.txt", machine: "lab", kind: "new-file" }, "Not sure it was done: check again"],
  ["a rename that would have to copy", host("rename", "cannot rename /srv/a: /mnt/a is on another file system on lab, and a rename does not copy"), { name: "a", machine: "lab", kind: "rename" }, "A rename can't copy to another disk"],
  ["a machine without mv", host("rename", "cannot rename /srv/a: lab has no mv, which this needs"), { name: "a", machine: "lab", kind: "rename" }, "lab has no mv"],
  ["a Windows machine", host("createFile", "cannot create /srv/a on lab: this needs sh on lab, and it has none (Windows machines cannot be changed from here)"), { name: "a", machine: "lab", kind: "new-file" }, "Windows machines can't be changed here"],
  ["a path a machine cannot use", host("createFile", "cannot create C:\\a: a machine's path has to start with / and name a file or a folder (a Windows path cannot be used)"), { name: "a", machine: "lab", kind: "new-file" }, "A machine can't use that path"],
  ["a rename onto a document still open", host("rename", "/tmp/b.txt is still open (its file was deleted): close it first"), { name: "b.txt", kind: "rename" }, "A document of that name is still open"],
  ["a rename of a file being saved", host("rename", "/tmp/a.txt is being saved: try again when it is done"), { name: "b.txt", kind: "rename" }, "It is being saved: try again shortly"],
  ["a file that cannot go to the trash", host("trash", "cannot move /tmp/a.txt to the trash: Permission denied (os error 13)"), { name: "a.txt", kind: "trash" }, "Permission denied"],
  ["a folder that cannot be opened", host("listDir", "cannot read /srv/x: Permission denied (os error 13)"), { name: "x", kind: "list" }, "Permission denied"],
  ["a failure it does not know, with the path left out", host("rename", "cannot rename /tmp/run/a.txt: Disk on fire (os error 99)"), { name: "b.txt", kind: "rename" }, "Disk on fire"],
  ["a failure with no cause to cut", host("listDir", "cannot open /srv/run/x"), { name: "x", kind: "list" }, "cannot open x"],
  ["a failure that is only a name", "boom", {}, "boom"],
  ["nothing said", undefined, {}, "Something went wrong"],
  ["nothing but the prefix", host("rename", ""), {}, "Something went wrong"],
];

for (const [what, error, at, want] of cases) {
  test(`${what}: ${want}`, () => {
    const got = reason(error, at);
    expect(got).toBe(want);
    expect(got.length).toBeLessThanOrEqual(LIMIT);
    expect(got).not.toContain("yuzora:files");
    expect(got).not.toMatch(/\//);
  });
}

test("a long name is cut in the middle, where its end (the extension) stays", () => {
  const name = `${"quarterly-".repeat(6)}report.final.xlsx`;
  const got = reason(host("createFile", `cannot create /x/${name}: File exists (os error 17)`), { name, kind: "new-file" });
  expect(got.length).toBeLessThanOrEqual(LIMIT);
  expect(got).toMatch(/^quarterly-.*….*\.xlsx already exists$/);
});

test("a long machine name is cut too", () => {
  const machine = "build-server-eu-west-1.internal";
  const got = reason(host("createFile", `cannot create /srv/n.txt on ${machine}: machine ${machine} is not connected`), { name: "n.txt", machine, kind: "new-file" });
  expect(got.length).toBeLessThanOrEqual(LIMIT);
  expect(got).toMatch(/^Can't reach b.*….+/);
});

test("what a failure it does not know says is cut at the limit, its start kept", () => {
  const got = reason(host("rename", `cannot rename /tmp/a.txt: ${"very ".repeat(30)}wrong`), { name: "b.txt", kind: "rename" });
  expect(got.length).toBe(LIMIT);
  expect(got.startsWith("very very")).toBe(true);
  expect(got.endsWith("…")).toBe(true);
});
