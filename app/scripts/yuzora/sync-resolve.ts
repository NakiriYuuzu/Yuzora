// yuzora(L9): the conflicts every herdr-app release brings to app/, each with one answer
// (docs/beta2/architecture.md, "上游同步流程"). scripts/yuzora/sync-pull runs it on a conflicted
// file; a conflict that is none of these stays in the file, markers and all, for a person.
//
//   bun scripts/yuzora/sync-resolve.ts <Cargo.toml|Cargo.lock|CHANGELOG.md|THIRD-PARTY-NOTICES> <file>
//
// Exit status: 0 when the file has no conflict left, 1 when some stay.

export type Conflict = { ours: string[]; theirs: string[] };
/** The lines that replace a conflict, or null to leave it as it is. */
export type Rule = (conflict: Conflict) => string[] | null;

const said = (lines: string[]) => lines.filter(line => line.trim() !== "");
const version = /^version = "[^"]+"$/;

export const rules: Record<string, Rule> = {
  // Upstream's release commit sets the package version; Yuzora's comes from the workspace.
  "Cargo.toml": ({ ours, theirs }) =>
    said(ours).join() === "version.workspace = true" && said(theirs).every(line => version.test(line)) ? ours : null,
  // Yuzora's side: cargo then adds what the merged Cargo.toml asks for (sync-pull runs it).
  "Cargo.lock": ({ ours }) => ours,
  // Both add entries at the top. Yuzora's stay first: the app shows the first as this version's notes.
  "CHANGELOG.md": ({ ours, theirs }) => {
    const entry = (lines: string[]) => lines.some(line => line.startsWith("## ["));
    if (!entry(ours) || !said(theirs)[0]?.startsWith("## [")) return null;
    return [...ours, ...(ours.at(-1)?.trim() === "" ? [] : [""]), ...theirs];
  },
  // Generated from Cargo.lock: sync-pull writes it again once the lock is settled.
  "THIRD-PARTY-NOTICES": ({ ours }) => ours,
};

/** `text` with each conflict the rule answers replaced, and how many it left. */
export function resolve(text: string, rule: Rule): { text: string; left: number } {
  const out: string[] = [];
  let left = 0;
  let open: { lines: string[]; ours: string[]; theirs: string[]; side: "ours" | "base" | "theirs" } | null = null;
  for (const line of text.split("\n")) {
    if (!open) {
      if (line.startsWith("<<<<<<< ")) open = { lines: [line], ours: [], theirs: [], side: "ours" };
      else out.push(line);
      continue;
    }
    open.lines.push(line);
    if (line.startsWith("||||||| ") && open.side === "ours") open.side = "base"; // diff3 style
    else if (line === "=======" && open.side !== "theirs") open.side = "theirs";
    else if (line.startsWith(">>>>>>> ") && open.side === "theirs") {
      const answer = rule({ ours: open.ours, theirs: open.theirs });
      if (!answer) left += 1;
      out.push(...(answer ?? open.lines));
      open = null;
    } else if (open.side !== "base") open[open.side].push(line);
  }
  if (open) {
    left += 1;
    out.push(...open.lines);
  }
  return { text: out.join("\n"), left };
}

if (import.meta.main) {
  const [kind, file] = process.argv.slice(2);
  const rule = rules[kind ?? ""];
  if (!rule || !file) {
    console.error(`usage: sync-resolve.ts <${Object.keys(rules).join("|")}> <file>`);
    process.exit(2);
  }
  const result = resolve(await Bun.file(file).text(), rule);
  await Bun.write(file, result.text);
  process.exit(result.left === 0 ? 0 : 1);
}
