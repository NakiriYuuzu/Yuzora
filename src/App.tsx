import { SftpHost, SshAuthenticationHost } from "@/workbench/SftpHost";
import { AppShell } from "@/app/AppShell";
import { FolderPickerHost } from "@/workbench/FolderPickerHost";
import { HostConnectionsBridge } from "@/workbench/HostConnectionsBridge";
import { ExternalChangeBridge } from "@/workbench/ExternalChangeBridge";
import { ExternalChangeResolver } from "@/workbench/ExternalChangeResolver";
import { GitBridge } from "@/workbench/GitBridge";
import { FileDropBridge } from "@/workbench/FileDropBridge";
import { HerdrBridge } from "@/workbench/HerdrBridge";
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
      <FolderPickerHost />
      <HostConnectionsBridge />
      <ExternalChangeBridge />
      <ExternalChangeResolver />
      <GitBridge />
      <FileDropBridge />
      <SessionRestoreBridge />
      <HerdrBridge />
      <SshBridge />
      <SshHostKeyHost />
      <SftpHost />
      <SshAuthenticationHost />
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
