// The pane a tab is about, with no imports (a test runs this as it is).

// The panes that show one file: their props name it (`path`), as yuzora-sidebar-files opens them.
const FILE_MODS = ["yuzora-editor", "yuzora-markdown", "yuzora-files-image"];

/** The pane a tab is named after: its agent's, else a terminal's, else its first. */
export function leadOf(mine) {
  return mine.find((p) => p.program?.agent) ?? mine.find((p) => p.terminal) ?? mine[0];
}

/** The path of the file a tab shows, when the pane it is named after shows one. */
export function tabFile(mine) {
  const lead = leadOf(mine);
  return (FILE_MODS.includes(lead?.mod) && lead.props?.path) || null;
}
