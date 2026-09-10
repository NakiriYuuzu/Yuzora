import { expect, it } from "vitest";
import { sampleQuery } from "./query";
it("projects and sorts selected sample columns with a row limit", () => {
  expect(
    sampleQuery("SELECT role, name FROM agents ORDER BY name DESC LIMIT 1;"),
  ).toEqual({
    columns: ["role", "name"],
    rows: [
      [
        { kind: "text", value: "Researcher" },
        { kind: "text", value: "Pi" },
      ],
    ],
  });
});
it("uses the selected table rather than returning agent rows for every query", () => {
  expect(
    sampleQuery('SELECT * FROM "main"."sessions" LIMIT 1000').columns,
  ).toEqual(["name", "workspace", "status"]);
});
it("rejects mutations and unsupported queries without pretending to execute them", () => {
  for (const sql of [
    "DELETE FROM agents",
    "SELECT password FROM agents",
    "SELECT * FROM private_data",
    "SELECT * FROM agents; DROP TABLE agents;",
  ])
    expect(() => sampleQuery(sql)).toThrow();
});
