import { gitStage, gitStatus, gitUnstage } from "@/lib/ipc";
import { afterEach, expect, it } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { invoke } from "@tauri-apps/api/core";
import { useDbStore } from "@/state/dbStore";
import { installDemoRuntime } from "./runtime";

afterEach(clearMocks);
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
