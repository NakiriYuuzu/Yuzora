/** Requires an explicitly authorized browser-recording session and a running Pages preview. */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const root = resolve(import.meta.dirname, "..");
const base = process.env.YUZORA_DEMO_URL ?? "http://127.0.0.1:4173/demo/";
const session = "yuzora-site-recording";
const cli = process.env.PLAYWRIGHT_CLI
  ? [process.env.PLAYWRIGHT_CLI]
  : ["bunx", "@playwright/cli"];
async function run(...args: string[]) {
  const child = Bun.spawn([...cli, `-s=${session}`, "--raw", ...args], {
    cwd: "/tmp",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (code !== 0 || stdout.includes("### Error"))
    throw new Error(stdout + stderr);
  return stdout;
}
await mkdir(resolve(root, "site-remotion/public/captures"), {
  recursive: true,
});
await mkdir(resolve(root, "output/playwright"), { recursive: true });
const actions = resolve(root, "output/playwright/site-demo-actions.js");
await writeFile(
  actions,
  (
    await readFile(resolve(root, "scripts/site-demo-actions.js.txt"), "utf8")
  ).replace("__REPO_ROOT__", JSON.stringify(root)),
);
await run("open", base, "--headed");
await run("resize", "1440", "900");
for (const lang of ["en", "zh"]) {
  for (const feature of ["ade-herdr", "terminal-git", "remote-db"]) {
    if (
      process.argv.length > 2 &&
      !process.argv.slice(2).includes(`${feature}-${lang}`)
    )
      continue;
    const url = new URL(base);
    url.search = new URLSearchParams({
      lang: lang === "en" ? "en" : "zh-TW",
      capture: feature,
      accent: "blue",
      scene:
        feature === "terminal-git"
          ? "git"
          : feature === "remote-db"
            ? "database"
            : "terminal",
    }).toString();
    await run("goto", url.href);
    await run(
      "video-start",
      resolve(root, `site-remotion/public/captures/${feature}-${lang}.webm`),
      "--size",
      "1440x900",
    );
    try {
      await run("run-code", "--filename", actions);
    } finally {
      await run("video-stop");
    }
    console.log(`Recorded ${feature}-${lang}`);
  }
}
await run("close");
