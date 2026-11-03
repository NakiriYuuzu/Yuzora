// Shared bits of the database panes: grid colors from the theme, and a
// one-line SQL highlighter for the query strip.
import { div } from "gpui-kit";
import { tokens } from "yuzora:core";

/** The DataGrid's colors, from the person's theme (and Yuzora's tokens for what it has no name for). */
export function gridColors(th) {
  const db = tokens().database;
  return {
    foreground: th.foreground, muted: th.muted, dim: th.dim, divider: th.divider ?? th.border,
    header: th.raised, hover: th.hover, selected: th.selected, accent: th.accent,
    editedRow: db.edited_row, warning: th.blocked, error: db.error, button: th.accent, buttonText: th.accent_foreground,
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

/** Spans for one line of SQL: keywords, strings, numbers, comments (tokens' sql colors, as the editor). */
export function highlight(sql, th) {
  const c = tokens().sql;
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
    const color = comment ? c.comment : str ? c.string : num ? c.number : word && KEYWORDS.has(word.toLowerCase()) ? c.keyword : null;
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
