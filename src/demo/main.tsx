import { installDemoRuntime } from "./runtime";
installDemoRuntime();
// Bootstrap the transport before importing any module that calls native IPC.
void import("./Demo").then(({ mountDemo }) => mountDemo());
