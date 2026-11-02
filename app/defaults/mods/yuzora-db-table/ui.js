// Shared bits of the database panes: grid colors from the theme, and a
// one-line SQL highlighter for the query strip.
import { div } from "gpui-kit";

/** The DataGrid's colors, from the person's theme. */
export function gridColors(th) {
  return {
    foreground: th.foreground, muted: th.muted, dim: th.dim, divider: th.divider ?? th.border,
    header: th.raised, hover: th.hover, selected: th.selected, accent: "#60a5fa",
    editedRow: "#1a2333", warning: th.blocked, error: "#f08a8a", button: "#60a5fa",
    buttonBorder: th.input, background: th.background,
  };
}

const KEYWORDS = new Set([
  "select", "from", "where", "order", "by", "group", "having", "limit", "offset", "top", "insert", "into", "values",
  "update", "set", "delete", "create", "alter", "drop", "table", "view", "index", "add", "column", "join", "left",
  "right", "inner", "outer", "full", "cross", "on", "as", "and", "or", "not", "in", "is", "null", "like", "ilike",
  "between", "exists", "distinct", "union", "all", "case", "when", "then", "else", "end", "asc", "desc", "with",
  "returning", "begin", "commit", "rollback", "truncate", "grant", "revoke", "primary", "key", "foreign",
  "references", "default", "constraint", "unique", "check", "exec", "execute", "declare",
]);

/** Spans for one line of SQL: keywords, strings, numbers, comments. */
export function highlight(sql, th) {
  const text = sql.replace(/\s+/g, " ").trim().slice(0, 600);
  const out = [];
  const re = /(--[^\n]*|\/\*[\s\S]*?\*\/)|('(?:[^']|'')*'?)|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z0-9_]*)|([^A-Za-z0-9_'\-/]+|[-/])/g;
  let m;
  let plain = "";
  const flush = () => {
    if (plain) out.push(div().flex_none().whitespace_nowrap().text_color(th.foreground).child(plain));
    plain = "";
  };
  while ((m = re.exec(text))) {
    const [all, comment, str, num, word] = m;
    const color = comment ? "#8b8f98" : str ? "#98c379" : num ? "#d19a66" : word && KEYWORDS.has(word.toLowerCase()) ? "#c678dd" : null;
    if (!color) {
      plain += all;
      continue;
    }
    flush();
    out.push(div().flex_none().whitespace_nowrap().text_color(color).child(all));
  }
  flush();
  return out;
}
