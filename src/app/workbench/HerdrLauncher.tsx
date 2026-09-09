import { openCreatedHerdrTabAndRequestName } from "@/lib/herdrTabActions";
import { Separator } from "@/components/ui/separator";

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  FolderOpen,
  FolderPlus,
  Layers,
  Plus,
  RefreshCw,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useHerdrStore } from "@/state/herdrStore";
import { sessionScope, parseRuntimeScope } from "@/lib/herdrProvider";
import { chooseWorkspaceFolder } from "@/state/folderPickerStore";
import { pickWorkspace } from "@/lib/workspaceActions";
import { workspacePathBasename } from "@/lib/paths";

export function HerdrLauncher({
  scope,
  onScopeChange,
}: {
  scope: string | null;
  onScopeChange: (scope: string | null) => void;
}) {
  const { t } = useTranslation("spaceTree");
  const { t: tw } = useTranslation("workbench");
  const sessions = useHerdrStore((s) => s.sessions);
  const selected = useHerdrStore((s) => s.selectedSessionName);
  const space = useHerdrStore((s) =>
    s.snapshot?.spaces.find((x) => x.id === s.selectedSpaceId),
  );
  const canCreateTerminal = useHerdrStore((s) => s.canCreateTerminal());
  const canCreateSpace = useHerdrStore((s) => s.canCreateSpace());
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<string | null>(null);
  const label = (id: string) => {
    const session = sessions.find((x) => sessionScope(x) === id);
    return session
      ? [session.hostLabel, session.name].filter(Boolean).join(" · ")
      : parseRuntimeScope(id).sessionName;
  };
  async function run(action: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="herdr-launcher">
        <div className="herdr-launcher-row">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                className="herdr-scope-button"
                aria-label={t("switchSession", {
                  session: scope ? label(scope) : "All",
                })}
                title={t("openMenu")}
              >
                <Layers />
                <span>
                  <small>Herdr</small>
                  <strong>{scope ? label(scope) : "All"}</strong>
                </span>
                <ChevronDown />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuLabel>{t("loadedSessions")}</DropdownMenuLabel>
              <DropdownMenuRadioGroup
                value={scope === null ? "all" : `session:${scope}`}
                onValueChange={(value) => {
                  if (value === "all") {
                    onScopeChange(null);
                    return;
                  }
                  const id = value.slice(8);
                  void run(async () => {
                    await useHerdrStore.getState().selectSession(id);
                    onScopeChange(id);
                  });
                }}
              >
                <DropdownMenuRadioItem value="all">All</DropdownMenuRadioItem>
                {sessions.filter((session) => session.running).map((session) => (
                  <DropdownMenuRadioItem
                    key={sessionScope(session)}
                    value={`session:${sessionScope(session)}`}
                  >
                    {label(sessionScope(session)!)}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
              <Separator />
              <DropdownMenuGroup>
                <DropdownMenuItem
                  disabled={busy || !space || !canCreateTerminal}
                  onSelect={() =>
                    void run(async () => {
                      const created = await useHerdrStore
                        .getState()
                        .createTerminalInSelectedSpace();
                      if (!created)
                        throw new Error(
                          useHerdrStore.getState().errorMessage ??
                            tw("herdrNav.createFailedUnknown"),
                        );
                      await openCreatedHerdrTabAndRequestName({
                        sessionName: created.herdrSessionId,
                        workspaceId: created.workspaceId,
                        terminalId: created.terminalId,
                        title: created.title,
                        paneId: created.paneId,
                        tabId: created.tabId,
                      });
                    })
                  }
                >
                  <Plus />
                  {tw("herdrNav.newTerminal")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  disabled={busy}
                  onSelect={() =>
                    void run(() => useHerdrStore.getState().refreshSessions())
                  }
                >
                  <RefreshCw />
                  {t("refreshSessions")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  disabled={busy}
                  onSelect={() => void run(() => pickWorkspace())}
                >
                  <FolderOpen />
                  {t("openFolder")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  disabled={busy || !selected || !canCreateSpace}
                  onSelect={() =>
                    void run(async () => {
                      if (!selected) return;
                      const path = await chooseWorkspaceFolder({
                        runtimeHostId: parseRuntimeScope(selected).hostId,
                      });
                      if (typeof path !== "string") return;
                      const result = await useHerdrStore
                        .getState()
                        .createSpaceFromFolder(
                          path,
                          workspacePathBasename(path),
                        );
                      if (!result.ok && !result.cancelled)
                        throw new Error(
                          result.error ?? tw("herdrNav.createFailedUnknown"),
                        );
                    })
                  }
                >
                  <FolderPlus />
                  {tw("herdrNav.createSpaceFromFolder")}
                </DropdownMenuItem>
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      {error && (
        <p className="herdr-launcher-notice" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
