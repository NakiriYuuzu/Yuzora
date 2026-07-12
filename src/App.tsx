import { AppShell } from "@/app/AppShell";
import { ExternalChangeBridge } from "@/workbench/ExternalChangeBridge";
import { ExternalChangeResolver } from "@/workbench/ExternalChangeResolver";
import { GitBridge } from "@/workbench/GitBridge";
import { LspBridge } from "@/workbench/LspBridge";
import { AgentBridge } from "@/workbench/AgentBridge";
import { ProcessBridge } from "@/workbench/ProcessBridge";
import { TerminalBridge } from "@/workbench/TerminalBridge";
import { SshBridge } from "@/workbench/SshBridge";
import { PerfBridge } from "@/workbench/PerfBridge";
import { SessionRestoreBridge } from "@/workbench/SessionRestoreBridge";
import { AskpassHost } from "@/workbench/AskpassHost";
import { ConfirmDialogHost } from "@/workbench/ConfirmDialogHost";
import { GitRollbackDialog } from "@/workbench/git/GitRollbackDialog";

function App() {
  return (
    <>
      <AppShell />
      <ExternalChangeBridge />
      <ExternalChangeResolver />
      <GitBridge />
      <LspBridge />
      <TerminalBridge />
      <AgentBridge />
      <ProcessBridge />
      <SshBridge />
      <PerfBridge />
      <SessionRestoreBridge />
      <AskpassHost />
      <ConfirmDialogHost />
      <GitRollbackDialog />
    </>
  );
}

export default App;
