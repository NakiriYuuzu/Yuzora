import type { DbValue } from "@/lib/types";

const tables: Record<string, { columns: string[]; rows: string[][] }> = {
  agents: {
    columns: ["name", "role", "status"],
    rows: [
      ["Claude", "Reviewer", "Done"],
      ["Codex", "Builder", "Working"],
      ["Pi", "Researcher", "Idle"],
    ],
  },
  sessions: {
    columns: ["name", "workspace", "status"],
    rows: [
      ["studio", "Evening Studio", "Running"],
      ["ideas", "Little Ideas", "Idle"],
    ],
  },
  workspaces: {
    columns: ["name", "path", "agents"],
    rows: [
      ["Evening Studio", "/demo/evening-studio", "2"],
      ["Little Ideas", "/demo/little-ideas", "1"],
    ],
  },
};

/** A deliberately small sample SELECT interpreter, never an SQL/database connection. */
export function sampleQuery(sql: string): {
  columns: string[];
  rows: DbValue[][];
} {
  const normalized = sql.replace(/["`[\]]/g, "").trim();
  const match =
    /^select\s+([\w*,\s]+?)\s+from\s+(?:main\.)?(\w+)(?:\s+order\s+by\s+(\w+)(?:\s+(asc|desc))?)?(?:\s+limit\s+(\d+))?\s*;?$/i.exec(
      normalized,
    );
  const table = match && tables[match[2].toLowerCase()];
  if (!match || !table)
    throw new Error(
      "Demo supports SELECT columns FROM agents, sessions or workspaces, with ORDER BY and LIMIT. No SQL is sent to a server.",
    );
  const columns =
    match[1].trim() === "*"
      ? table.columns
      : match[1].split(",").map((value) => value.trim().toLowerCase());
  if (
    columns.some((name) => !table.columns.includes(name)) ||
    (match[3] && !table.columns.includes(match[3].toLowerCase()))
  )
    throw new Error(`Available demo columns: ${table.columns.join(", ")}`);
  let rows = [...table.rows];
  if (match[3]) {
    const index = table.columns.indexOf(match[3].toLowerCase());
    rows.sort(
      (a, b) =>
        a[index].localeCompare(b[index]) *
        (match[4]?.toLowerCase() === "desc" ? -1 : 1),
    );
  }
  if (match[5]) rows = rows.slice(0, Number(match[5]));
  return {
    columns,
    rows: rows.map((row) =>
      columns.map((name) => ({
        kind: "text",
        value: row[table.columns.indexOf(name)],
      })),
    ),
  };
}
