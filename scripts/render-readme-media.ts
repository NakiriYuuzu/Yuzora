/** Render README media from the same AppShell recordings used by Pages. */
import { copyFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const remotionRoot = resolve(root, "site-remotion");
async function render(args: string[]) {
  const renderProcess = Bun.spawn(["bunx", "remotion", ...args, "--log=error"], {
    cwd: remotionRoot,
    stdout: "inherit",
    stderr: "inherit",
  });
  if ((await renderProcess.exited) !== 0) throw new Error(`Remotion failed: ${args.join(" ")}`);
}

for (const lang of ["en", "zh"]) {
  for (const [feature, frame] of [["ade-herdr", 90], ["terminal-git", 160], ["remote-db", 60]] as const) {
    const output = resolve(root, `docs/readme/${feature}-${lang}.png`);
    await render(["still", "src/index.ts", `${feature}-${lang}`, output, `--frame=${frame}`]);
    const poster = feature === "ade-herdr" ? "ade-herdr-runtime" : feature;
    await copyFile(output, resolve(root, `site/assets/${poster}-${lang}.png`));
  }
  await render([
    "render", "src/index.ts", `readme-tour-${lang}`,
    resolve(root, `docs/readme/hero-${lang}.gif`),
    "--codec=gif", "--every-nth-frame=5", "--scale=0.6111111111111112", "--concurrency=4",
  ]);
  console.log(`Rendered README media (${lang})`);
}
