import { SpaceAgentTree } from "./SpaceAgentTree";
import "./space-agent-sidebar.css";

/** Keep mounted across tool modes so browsing and keyboard context survive. */
export function SpaceAgentSidebar() {
  return <SpaceAgentTree />;
}
