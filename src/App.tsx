import { AppShell } from "@/app/AppShell";
import { ExternalChangeBridge } from "@/workbench/ExternalChangeBridge";
import { ExternalChangeResolver } from "@/workbench/ExternalChangeResolver";
import { GitBridge } from "@/workbench/GitBridge";
import { FileDropBridge } from "@/workbench/FileDropBridge";
import { LspBridge } from "@/workbench/LspBridge";
import { HerdrBridge } from "@/workbench/HerdrBridge";
import { ProcessBridge } from "@/workbench/ProcessBridge";
import { SshBridge } from "@/workbench/SshBridge";
import { SshHostKeyHost } from "@/workbench/SshHostKeyHost";
import { PerfBridge } from "@/workbench/PerfBridge";
import { SessionRestoreBridge } from "@/workbench/SessionRestoreBridge";
import { AskpassHost } from "@/workbench/AskpassHost";
import { AppDialogHost } from "@/workbench/AppDialogHost";
import { WorkspaceTrustHost } from "@/workbench/WorkspaceTrustHost";
import { ConfirmDialogHost } from "@/workbench/ConfirmDialogHost";
import { TextInputDialogHost } from "@/workbench/TextInputDialogHost";
import { GitRollbackDialog } from "@/workbench/git/GitRollbackDialog";

function App() {
  return (
    <>
      <AppShell />
      <ExternalChangeBridge />
      <ExternalChangeResolver />
      <GitBridge />
      <FileDropBridge />
      <LspBridge />
      <SessionRestoreBridge />
      <HerdrBridge />
      <ProcessBridge />
      <SshBridge />
      <SshHostKeyHost />
      <PerfBridge />
      <AskpassHost />
      <WorkspaceTrustHost />
      <AppDialogHost />
      <ConfirmDialogHost />
      <TextInputDialogHost />
      <GitRollbackDialog />
    </>
  );
}

export default App;
