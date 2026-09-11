import { fsCreateDir, fsCreateFile, listDir, openFile, saveFile, gitStage, gitStatus, gitUnstage } from "@/lib/ipc";
import { afterEach, expect, it } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { invoke } from "@tauri-apps/api/core";
import { useDbStore } from "@/state/dbStore";
import { files, ROOT, installDemoRuntime } from "./runtime";

const initialFiles = { ...files };
afterEach(() => {
  for (const key of Object.keys(files)) delete files[key];
  Object.assign(files, initialFiles);
});

afterEach(clearMocks);
it("creates and lists empty demo directories, then opens and saves a new empty file", async () => {
  installDemoRuntime();
  await fsCreateDir(ROOT, `${ROOT}/empty`);
  expect(await listDir(ROOT)).toContainEqual(expect.objectContaining({ name: "empty", isDir: true, path: `${ROOT}/empty` }));
  expect(await listDir(`${ROOT}/empty`)).toEqual([]);
  await fsCreateFile(ROOT, `${ROOT}/empty/note.txt`);
  expect(await openFile(`${ROOT}/empty/note.txt`)).toMatchObject({ kind: "full", content: "", size: 0 });
  await saveFile(`${ROOT}/empty/note.txt`, "saved note");
  expect(await openFile(`${ROOT}/empty/note.txt`)).toMatchObject({ content: "saved note" });
  expect(await listDir(`${ROOT}/empty`)).toContainEqual(expect.objectContaining({ name: "note.txt", isDir: false }));
});

it("rejects duplicate demo paths without overwriting files or directories", async () => {
  installDemoRuntime();
  const original = files["README.md"];
  await expect(fsCreateFile(ROOT, `${ROOT}/README.md`)).rejects.toThrow("already exists");
  await expect(fsCreateDir(ROOT, `${ROOT}/README.md`)).rejects.toThrow("already exists");
  await expect(fsCreateFile(ROOT, `${ROOT}/src`)).rejects.toThrow("already exists");
  await fsCreateDir(ROOT, `${ROOT}/empty`);
  await expect(fsCreateDir(ROOT, `${ROOT}/empty`)).rejects.toThrow("already exists");
  expect(files["README.md"]).toBe(original);
});

it("keeps demo creation inside its workspace and refuses file parents", async () => {
  installDemoRuntime();
  for (const path of ["/outside.txt", `${ROOT}/../escape.txt`, `${ROOT}-other/a.txt`, `${ROOT}/README.md/child.txt`]) {
    await expect(fsCreateFile(ROOT, path)).rejects.toThrow();
  }
  await expect(fsCreateDir("/different-workspace", `${ROOT}/empty`)).rejects.toThrow();
  await fsCreateFile(ROOT, `${ROOT}/nested/deep/new.txt`);
  expect(await listDir(`${ROOT}/nested`)).toContainEqual(expect.objectContaining({ name: "deep", isDir: true }));
  expect(await listDir(`${ROOT}/nested/deep`)).toContainEqual(expect.objectContaining({ name: "new.txt", isDir: false }));
});
it("loads sample profiles through the actual database store without an error banner", async () => {
  installDemoRuntime();
  useDbStore.setState({ saved: [], profilesLoaded: false, profileError: null });
  await useDbStore.getState().initializeProfiles();
  expect(useDbStore.getState().profileError).toBeNull();
  expect(useDbStore.getState().saved[0]).toMatchObject({
    id: "demo-db",
    name: "Studio · SQLite",
    kind: "sqlite",
  });
});
it("rejects unsupported commands at the demo transport boundary", async () => {
  installDemoRuntime();
  await expect(
    invoke("git_push_cmd", { path: "/demo/evening-studio" }),
  ).rejects.toThrow("Demo data stays in memory");
});

it("refreshes Git status through the real IPC wrappers after staging sample files", async () => {
  installDemoRuntime();
  await gitStage("/demo/evening-studio", ["src/App.tsx"]);
  expect((await gitStatus("/demo/evening-studio")).staged).toContainEqual({
    path: "src/App.tsx",
    origPath: null,
    status: "M",
  });
  await gitUnstage("/demo/evening-studio", ["src/App.tsx"]);
  expect((await gitStatus("/demo/evening-studio")).staged).toHaveLength(0);
});
