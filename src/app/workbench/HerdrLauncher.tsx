import { useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  FolderPlus,
  Layers,
  Plus,
  RefreshCw,
  Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
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
import { sessionScope } from "@/lib/herdrProvider";
import { runtimeSessionLabel } from "./spaceTreeIdentity";
import { HerdrSessionPicker } from "./HerdrSessionPicker";

export function HerdrLauncher({
  scope,
  onScopeChange,
  onCreateSpace,
  creatingSpace,
  viewSwitcher,
  onOpenTools,
}: {
  viewSwitcher: ReactNode;
  /** Omitted while no Session is selected: tools never fall back to a default Session. */
  onOpenTools?: () => void;
  scope: string | null;
  onScopeChange: (scope: string | null) => void;
  onCreateSpace: (sessionName: string) => Promise<void>;
  creatingSpace: boolean;
}) {
  const { t } = useTranslation("spaceTree");
  const sessions = useHerdrStore((s) => s.sessions);
  const selected = useHerdrStore((s) => s.selectedSessionName);
  const canCreateSpace = useHerdrStore((s) => s.canCreateSpace());
  const ready = useHerdrStore((s) => s.connectionState === "ready" && !s.errorMessage && s.capabilities?.server.compatible !== false);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<string | null>(null);
  const [sessionPickerOpen, setSessionPickerOpen] = useState(false);
  const addTrigger = useRef<HTMLButtonElement>(null);
  const runningSessions = sessions.filter((session) => session.running);
  const label = (id: string) => {
    const session = sessions.find((x) => sessionScope(x) === id);
    return runtimeSessionLabel(id, session);
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
  function openSessionPicker() {
    setError(null);
    setSessionPickerOpen(true);
  }
  return (
    <>
      <div className="herdr-launcher" role="group" aria-label={t("spacesAndAgents", { ns: "workbenchShell" })}>
        <div className="herdr-launcher-row">
          {viewSwitcher}
          <Button variant="ghost" size="icon-sm" aria-label={t("title", { ns: "herdrTools" })} title={t("title", { ns: "herdrTools" })} disabled={!onOpenTools} onClick={onOpenTools}>
            <Wrench aria-hidden="true" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button ref={addTrigger} variant="ghost" size="icon-sm" aria-label={t("addMenu")} title={t("addMenu")}>
                <Plus aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" onCloseAutoFocus={(event) => {
              if (sessionPickerOpen) event.preventDefault();
            }}>
              <DropdownMenuGroup>
                <DropdownMenuItem
                  disabled={busy || creatingSpace || !ready || !selected || !canCreateSpace}
                  onSelect={() => void run(async () => {
                    if (selected) await onCreateSpace(selected);
                  })}
                >
                  <FolderPlus aria-hidden="true" />
                  {t("openFolderAndCreateSpace")}
                </DropdownMenuItem>
                <DropdownMenuItem disabled={busy || creatingSpace} onSelect={openSessionPicker}>
                  <Layers aria-hidden="true" />
                  {t("addSession")}
                </DropdownMenuItem>
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="herdr-scope-button"
              aria-label={t("switchSession", {
                session: scope ? label(scope) : "All",
              })}
              title={scope ? label(scope) : t("allSessions")}
              disabled={busy}
            >
              <Layers aria-hidden="true" />
              <span>
                <strong>{scope ? label(scope) : t("allSessions")}</strong>
              </span>
              <ChevronDown aria-hidden="true" />
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
              <DropdownMenuRadioItem value="all">{t("allSessions")}</DropdownMenuRadioItem>
              {runningSessions.map((session) => (
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
                disabled={busy}
                onSelect={() =>
                  void run(() => useHerdrStore.getState().refreshSessions())
                }
              >
                <RefreshCw aria-hidden="true" />
                {t("refreshSessions")}
              </DropdownMenuItem>
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {sessionPickerOpen && <HerdrSessionPicker
        initialSession={scope ?? selected}
        onSelect={(sessionName) => { onScopeChange(sessionName); setSessionPickerOpen(false); }}
        onClose={() => setSessionPickerOpen(false)}
        returnFocusRef={addTrigger}
      />}
      {error && !sessionPickerOpen && (
        <p className="herdr-launcher-notice" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
