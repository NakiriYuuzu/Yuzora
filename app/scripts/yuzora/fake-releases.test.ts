import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { serve } from "./fake-releases";

let server: ReturnType<typeof serve> | undefined;
afterEach(() => server?.stop(true));

describe("the stand-in for GitHub's releases", () => {
  test("lists the folder as one release and serves its files where the app looks", async () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-fake-"));
    writeFileSync(join(dir, "yuzora-update.json"), '{"a":1}');
    writeFileSync(join(dir, "yuzora-update.json.sig"), "sig");
    const seen: string[] = [];
    server = serve(dir, "0.1.0", 0, (line) => seen.push(line));
    const base = `http://127.0.0.1:${server.port}`;
    const list = (await (await fetch(`${base}/repos/NakiriYuuzu/Yuzora/releases?per_page=100`)).json()) as any[];
    expect(list[0]).toMatchObject({ tag_name: "v0.1.0", draft: false, prerelease: false });
    expect(list[0].assets.map((a: any) => a.name)).toEqual(["yuzora-update.json", "yuzora-update.json.sig"]);
    expect(list[0].assets[0].browser_download_url).toBe(`${base}/NakiriYuuzu/Yuzora/releases/download/v0.1.0/yuzora-update.json`);
    expect(await (await fetch(list[0].assets[0].browser_download_url)).text()).toBe('{"a":1}');
    expect(seen[0]).toBe("GET /repos/NakiriYuuzu/Yuzora/releases?per_page=100");
  });

  test("a pre-release version is listed as one; other files and paths are 404", async () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-fake-"));
    writeFileSync(join(dir, "a.txt"), "a");
    server = serve(dir, "0.1.1-beta.1");
    const base = `http://127.0.0.1:${server.port}`;
    expect(((await (await fetch(`${base}/repos/NakiriYuuzu/Yuzora/releases`)).json()) as any[])[0].prerelease).toBe(true);
    for (const path of ["/NakiriYuuzu/Yuzora/releases/download/v0.1.1-beta.1/missing", "/NakiriYuuzu/Yuzora/releases/download/v0.1.1-beta.1/..%2Fa.txt", "/NakiriYuuzu/Yuzora/releases/download/v9.9.9/a.txt", "/elsewhere"]) {
      expect((await fetch(`${base}${path}`)).status).toBe(404);
    }
    expect((await fetch(`${base}/NakiriYuuzu/Yuzora/releases/download/v0.1.1-beta.1/a.txt`)).status).toBe(200);
  });
});
