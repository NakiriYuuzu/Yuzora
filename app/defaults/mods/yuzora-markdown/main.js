// A Markdown pane for one file: the rendered preview, or its source in the
// editor, live with every other editor of the file. Props {path, on?}. Owned by lane L5.
// The pane itself is doc-pane.js (shared with yuzora-editor).
import { DocPane } from "./doc-pane.js";

export default class MarkdownPane extends DocPane {
  kind() { return "markdown"; }
}
