import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import "@/lib/i18n";
import "@/styles.css";
import "@/theme/system-tone.css";
import "@xterm/xterm/css/xterm.css";
import { AppShell } from "@/app/AppShell";
import { AppDialogHost } from "@/workbench/AppDialogHost";
import { ConfirmDialogHost } from "@/workbench/ConfirmDialogHost";
import { TextInputDialogHost } from "@/workbench/TextInputDialogHost";
import { Button } from "@/components/ui/button";
import { SpaceCharacter } from "@/app/workbench/SpaceCharacter";
import { useWorkspaceStore } from "@/state/workspaceStore";
import { useHerdrStore } from "@/state/herdrStore";
import { useDbStore, type DbQueryState } from "@/state/dbStore";
import type { DbConnectionGeneration } from "@/lib/types";
import { useGitStore } from "@/state/gitStore";
import { useSftpStore } from "@/state/sftpStore";
import { useUiStore } from "@/state/uiStore";
import { normalizeHerdrSnapshot } from "@/lib/herdrNormalize";
import { isAccentPreference } from "@/theme/accent";
import type { HerdrCapabilities } from "@/lib/herdrTypes";
import { APPEARANCE_SETTINGS_STORAGE_KEY } from "@/app/workbench/settingsStorage";
import i18n from "@/lib/i18n";
import {
  ROOT,
  sessions,
  capabilities,
  snapshot,
  status,
  branches,
  environment,
  dbProfile,
  dbTables,
} from "./runtime";
import "./demo.css";

function scene(name: string) {
  const ui = useUiStore.getState(),
    workspace = useWorkspaceStore.getState();
  ui.setSettingsOpen(false);
  if (name === "appearance") {
    ui.openSettings("appearance");
    return;
  }
  if (name === "git") {
    ui.setMode("git");
    ui.setGitPanelTab("local");
    ui.selectGitFile("src/App.tsx", false);
    return;
  }
  if (name === "database") {
    ui.setMode("database");
    return;
  }
  ui.setMode("files");
  if (name === "editor") workspace.openTab(`${ROOT}/src/App.tsx`);
  else
    workspace.openHerdrTerminalPage({
      herdrSessionId: "studio",
      terminalId: "term-build",
      paneId: "pane-build",
      herdrTabId: "build",
      herdrWorkspaceId: "studio",
      title: "Codex · build",
    });
}
// This standalone page exports its bootstrap alongside the page component.
// eslint-disable-next-line react-refresh/only-export-components
function Demo() {
  const { i18n: language } = useTranslation();
  const zh = language.language !== "en";
  const [notice, setNotice] = useState(false);
  useEffect(
    () =>
      useSftpStore.subscribe((state, previous) => {
        if (state.panelOpen && !previous.panelOpen) {
          setNotice(true);
          useSftpStore.getState().setPanelOpen(false);
        }
      }),
    [],
  );
  useEffect(() => {
    const listener = () => setNotice(true);
    window.addEventListener("demo-unavailable", listener);
    return () => window.removeEventListener("demo-unavailable", listener);
  }, []);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(false), 4500);
    return () => clearTimeout(timer);
  }, [notice]);
  return (
    <>
      <header className="demo-toolbar">
        <a href="../">← Yuzora</a>
        <span className="demo-toolbar-bot">
          <SpaceCharacter
            character={{ shell: "cloud", face: "smile", detail: "freckles" }}
          />
        </span>
        <strong>{zh ? "互動 Demo" : "Interactive demo"}</strong>
        <span className="demo-toolbar-note">
          {zh
            ? "範例資料 · 操作只在此分頁"
            : "Sample data · changes stay in this tab"}
        </span>
        <nav aria-label="Demo scenes">
          {[
            ["terminal", zh ? "Agents" : "Agents"],
            ["editor", zh ? "編輯器" : "Editor"],
            ["git", "Git diff"],
            ["database", zh ? "資料庫" : "Database"],
            ["appearance", zh ? "主題" : "Theme"],
          ].map(([id, label]) => (
            <Button
              key={id}
              variant="ghost"
              size="sm"
              data-demo-scene={id}
              onClick={() => scene(id)}
            >
              {label}
            </Button>
          ))}
        </nav>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void language.changeLanguage(zh ? "en" : "zh-TW")}
        >
          {zh ? "EN" : "中文"}
        </Button>
      </header>
      <div className="demo-workbench">
        <AppShell />
        <AppDialogHost />
        <ConfirmDialogHost />
        <TextInputDialogHost />
      </div>
      {notice && (
        <div className="demo-notice" role="status">
          {zh
            ? "此功能請在桌面 App 體驗；Demo 可操作編輯器、Git diff、主題與範例終端機。"
            : "Try this feature in the desktop app. Explore the editor, Git diff, themes and sample terminal here."}
        </div>
      )}
    </>
  );
}
export async function mountDemo() {
  const params = new URLSearchParams(location.search);
  await i18n.changeLanguage(params.get("lang") === "en" ? "en" : "zh-TW");
  const accent = params.get("accent");
  localStorage.setItem(
    APPEARANCE_SETTINGS_STORAGE_KEY,
    JSON.stringify({
      theme: params.get("theme") === "dark" ? "dark" : "light",
      accent: isAccentPreference(accent) ? accent : "blue",
      leftSidebarBackground: true,
      rightSidebarBackground: true,
    }),
  );
  const normalized = normalizeHerdrSnapshot(snapshot, "studio");
  const caps = capabilities as unknown as HerdrCapabilities;
  useHerdrStore.setState({
    sessions,
    selectedSessionName: "studio",
    selectedSpaceId: "studio",
    connectionState: "ready",
    capabilities: caps,
    snapshot: normalized,
    runtimesBySession: {
      studio: {
        connectionState: "ready",
        capabilities: caps,
        snapshot: normalized,
        errorMessage: null,
        worktreeInventory: null,
      },
    },
    selectedSpaceBySession: { studio: "studio" },
  });
  useWorkspaceStore.getState().setWorkspace(ROOT, "demo-workspace");
  useWorkspaceStore.getState().markSessionRestoreReady();
  useGitStore.setState({ environment, status, branches });
  const query: DbQueryState = {
    sql: "SELECT name, role, status\nFROM agents\nORDER BY name;",
    running: false,
    result: null,
    error: null,
    elapsedMs: null,
    lastSql: null,
    sortBy: null,
    sortBaseRows: null,
    parseError: null,
    runGroup: null,
  };
  useDbStore.setState({
    saved: [dbProfile],
    profilesLoaded: true,
    activeDescriptorId: dbProfile.id,
    activeConnId: "demo-connection",
    liveMru: [dbProfile.id],
    connections: [
      {
        connId: "demo-connection",
        connectionGeneration: "demo-generation" as DbConnectionGeneration,
        kind: "sqlite",
        name: dbProfile.name,
        descriptorId: dbProfile.id,
        targetKey: dbProfile.targetKey,
        title: dbProfile.path,
      },
    ],
    sessions: {
      [dbProfile.id]: {
        descriptorId: dbProfile.id,
        connId: "demo-connection",
        status: "connected",
        error: null,
      },
    },
    tableBuckets: { [dbProfile.id]: dbTables },
    tables: { "demo-connection": dbTables },
    queryBuckets: { [dbProfile.id]: query },
    queries: { "demo-connection": query },
  });
  useWorkspaceStore.getState().openTab(`${ROOT}/README.md`);
  scene(params.get("scene") ?? "terminal");
  createRoot(document.getElementById("root")!).render(<Demo />);
}
