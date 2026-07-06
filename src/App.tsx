import { AppShell } from "@/app/AppShell";
import { ExternalChangeBridge } from "@/workbench/ExternalChangeBridge";
import { ExternalChangeResolver } from "@/workbench/ExternalChangeResolver";
import { GitBridge } from "@/workbench/GitBridge";
import { LspBridge } from "@/workbench/LspBridge";
import { AgentBridge } from "@/workbench/AgentBridge";
import { ProcessBridge } from "@/workbench/ProcessBridge";
import { TerminalBridge } from "@/workbench/TerminalBridge";
import { AskpassHost } from "@/workbench/AskpassHost";

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
      <AskpassHost />
    </>
  );
}

export default App;
