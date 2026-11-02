// A code editor pane for one file: props {path, on?, line?, column?}. Owned by lane L5.
// The pane itself is doc-pane.js (shared with yuzora-markdown).
import { DocPane } from "./doc-pane.js";

export default class EditorPane extends DocPane {
  kind() { return "editor"; }
}
