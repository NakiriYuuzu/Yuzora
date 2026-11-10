// What a person is told when a change or a listing fails. yuzora-files reports a failure as one sentence
// (fsops.rs, remote_entry.rs, docs.rs) and the engine puts the call's name in front of it. The cause is found here,
// once, and written first: a toast has one line for its body and cuts what does not fit.
import { t } from "yuzora:core";

// A toast is 340 px wide, 284 of them for 12 px text: about 40 characters of the interface font, with some to spare.
export const LIMIT = 40;
const NAME_MAX = 20, MACHINE_MAX = 12;

const cut = (text, max) => (text.length <= max ? text : `${text.slice(0, max - 1)}…`);
// The end of a name stays: it is what tells names apart.
const short = (text, max) => (text.length <= max ? text : `${text.slice(0, max - 8)}…${text.slice(-7)}`);
// A path in a message, as the name it ends in.
const PATH = /(?<!\w)(?:[A-Za-z]:)?[\\/](?:[^\s\\/:;,]+[\\/])*([^\s\\/:;,]+)/g;

/**
 * The cause of `error` (what a host call rejected with) in a line of at most LIMIT characters.
 * `name`: the name the change was for; `machine`: where, null for this one; `kind`: the toast it is for
 * ("new-file", "new-folder", "rename", "trash", "reveal", "list").
 */
export function reason(error, { name = "", machine = null, kind = "" } = {}) {
  const text = String(error?.message ?? error ?? "").replace(/^`[^`]*`:\s*/, "").trim();
  const there = short(machine ?? "", MACHINE_MAX) || "the machine";
  const tell = (key, vars) => cut(t(`reason.${key}`, vars), LIMIT);
  const making = kind === "new-file" || kind === "new-folder";
  if (/already exists$|File exists|The file exists/i.test(text)) return tell("exists", { name: short(name, NAME_MAX) });
  if (/is still open/.test(text)) return tell("open");
  if (/is being saved/.test(text)) return tell("saving");
  if (/not connected|offline|unreachable|connection (?:refused|reset|closed|lost)|could not connect/i.test(text)) return tell("unreachable", { machine: there });
  if (/took longer than/.test(text)) return tell("timed", { machine: there });
  if (/may or may not/.test(text)) return tell("unsure");
  if (/does not exist on|no longer there|No such file|cannot find the (?:file|path)/i.test(text)) {
    return tell(making || /does not exist on/.test(text) ? "no_folder" : "gone");
  }
  if (/another file system/.test(text)) return tell("other_disk");
  const tool = text.match(/has no (\S+?), which this needs/)?.[1];
  if (tool) return tell("no_tool", { machine: there, tool });
  if (/this needs sh on/.test(text)) return tell("no_shell");
  if (/path has to start with/.test(text)) return tell("bad_path");
  if (/you may not change|Permission denied|Access is denied/i.test(text)) return tell(making || kind === "rename" ? "writable" : "denied");
  if (/Read-only file system/.test(text)) return tell("read_only");
  if (/No space left|not enough space/i.test(text)) return tell("full");
  if (/name too long/i.test(text)) return tell("too_long");
  if (/Not a directory/.test(text)) return tell("not_dir");
  const cause = text.replace(PATH, "$1").replace(/\s*\(os error \d+\)\.?$/, "").replace(/^cannot [^:]+: /, "");
  return cause ? cut(cause, LIMIT) : tell("unknown");
}
