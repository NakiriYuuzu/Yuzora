import { SftpHost, SshAuthenticationHost } from "@/workbench/SftpHost";
import { AppShell } from "@/app/AppShell";
import { FolderPickerHost } from "@/workbench/FolderPickerHost";
import { HostConnectionsBridge } from "@/workbench/HostConnectionsBridge";
import { WorkspaceResourcesBridge } from "@/workbench/WorkspaceResourcesBridge";
import { ExternalChangeBridge } from "@/workbench/ExternalChangeBridge";
import { ExternalChangeResolver } from "@/workbench/ExternalChangeResolver";
import { GitBridge } from "@/workbench/GitBridge";
import { FileDropBridge } from "@/workbench/FileDropBridge";
import { WorkbenchKeyboardBridge } from "@/workbench/WorkbenchKeyboardBridge";
import { WorkbenchFocusBridge } from "@/workbench/WorkbenchFocusBridge";
import { HerdrBridge } from "@/workbench/HerdrBridge";
import { HerdrToolsHost } from "@/workbench/HerdrToolsHost";
import { HerdrNotificationBridge } from "@/workbench/HerdrNotificationBridge";
import { ToasterHost } from "@/workbench/ToasterHost";
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
      <WorkspaceResourcesBridge />
      <ExternalChangeBridge />
      <ExternalChangeResolver />
      <GitBridge />
      <FileDropBridge />
      <SessionRestoreBridge />
      <HerdrBridge />
      <HerdrToolsHost />
      <HerdrNotificationBridge />
      <ToasterHost />
      <WorkbenchFocusBridge />
      <WorkbenchKeyboardBridge />
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
