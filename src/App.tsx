import { AppShell } from "@/app/AppShell";
import { ExternalChangeBridge } from "@/workbench/ExternalChangeBridge";
import { ExternalChangeResolver } from "@/workbench/ExternalChangeResolver";
import { GitBridge } from "@/workbench/GitBridge";
import { LspBridge } from "@/workbench/LspBridge";
import { AskpassHost } from "@/workbench/AskpassHost";

function App() {
  return (
    <>
      <AppShell />
      <ExternalChangeBridge />
      <ExternalChangeResolver />
      <GitBridge />
      <LspBridge />
      <AskpassHost />
    </>
  );
}

export default App;
