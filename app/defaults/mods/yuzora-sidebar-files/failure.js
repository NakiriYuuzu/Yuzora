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

// The entry a copy was refused inside the tree it copies: the name the path before the first "Permission denied" ends
// in, unless that is the item copied or the copy's hidden folder. GNU cp quotes the path; BSD cp and the copy made
// here end it with ": ".
function refusedIn(text, name) {
  const before = text.slice(0, text.search(/Permission denied|Access is denied/i)).replace(/:\s*$/, "");
  const quoted = before.match(/['‘]([^'’]*)['’][^'‘’]*$/), at = before.lastIndexOf(": ");
  const path = quoted ? quoted[1] : at < 0 ? "" : before.slice(at + 2);
  const last = /[\\/]/.test(path) ? path.split(/[\\/]/).pop() : "";
  return !last || last === name || /^(?:\.yuzora-save-)?[0-9a-f]{32}$/.test(last) ? "" : last;
}

/**
 * The cause of `error` (what a host call rejected with) in a line of at most LIMIT characters.
 * `name`: the name the change was for; `machine`: where, null for this one; `kind`: the toast it is for
 * ("new-file", "new-folder", "rename", "copy", "trash", "reveal", "list").
 */
export function reason(error, { name = "", machine = null, kind = "" } = {}) {
  const text = String(error?.message ?? error ?? "").replace(/^`[^`]*`:\s*/, "").trim();
  const there = short(machine ?? "", MACHINE_MAX) || "the machine";
  const tell = (key, vars) => cut(t(`reason.${key}`, vars), LIMIT);
  const making = kind === "new-file" || kind === "new-folder";
  if (/already exists$|File exists|The file exists/i.test(text)) return tell("exists", { name: short(name, NAME_MAX) });
  if (/is still open/.test(text)) return tell("open");
  if (/is being saved/.test(text)) return tell("saving");
  if (/cannot go into itself/.test(text)) return tell("into_itself");
  if (/folders inside each other/.test(text)) return tell("deep");
  if (/can take it to a trash/.test(text)) return tell("no_trash", { machine: there });
  if (/partial copy may be left/.test(text)) return tell("partial");
  // A copy's scratch folder that someone else changed (copy.sh, fsops::anchored): its name says nothing to a person.
  if (/is not the folder (?:just made|the copy was made in)/.test(text)) return tell("scratch_changed");
  // trash.sh could not write the .trashinfo; what the shell said follows, and a full disk is the likely cause.
  if (/trash information for .* could not be written/.test(text)) return tell(/No space left|Disk quota/i.test(text) ? "full" : "trash_info");
  if (/not connected|offline|unreachable|connection (?:refused|reset|closed|lost)|could not connect|lost the line/i.test(text)) return tell("unreachable", { machine: there });
  if (/took longer than/.test(text)) return tell("timed", { machine: there });
  if (/may or may not/.test(text)) return tell("unsure");
  if (/does not exist on|no longer there|No such file|cannot find the (?:file|path)/i.test(text)) {
    return tell(making || /does not exist on/.test(text) ? "no_folder" : "gone");
  }
  if (/another file system/.test(text)) return tell(kind === "trash" ? "trash_disk" : "other_disk");
  // The system would not rename it where it is (a folder out of an image layer, a bind mount): mv's or perl's own words.
  if (/cross-device link/i.test(text)) return tell("no_rename");
  if (/copied instead of renamed/.test(text)) return tell("copied");
  const tool = text.match(/has no (\S+?), which this needs/)?.[1];
  if (tool) return tell("no_tool", { machine: there, tool });
  if (/this needs sh on/.test(text)) return tell("no_shell");
  if (/path has to start with/.test(text)) return tell("bad_path");
  if (/you may not change/.test(text)) return tell("writable");
  if (/Permission denied|Access is denied/i.test(text)) {
    if (making || kind === "rename") return tell("writable");
    const entry = kind === "copy" ? refusedIn(text, name) : "";
    return entry ? tell("denied_at", { name: short(entry, NAME_MAX) }) : tell("denied");
  }
  if (/Read-only file system/.test(text)) return tell("read_only");
  if (/No space left|not enough space/i.test(text)) return tell("full");
  if (/name too long/i.test(text)) return tell("too_long");
  if (/Not a directory/.test(text)) return tell("not_dir");
  if (/is left in place/.test(text)) return tell("left");
  const cause = text.replace(PATH, "$1").replace(/\s*\(os error \d+\)\.?$/, "").replace(/^cannot [^:]+: /, "");
  return cause ? cut(cause, LIMIT) : tell("unknown");
}
