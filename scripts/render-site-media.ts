/** Render the captured, current UI through Remotion; run record-site-demo first. */
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";
const root = resolve(import.meta.dirname, "..");
const remotionRoot = resolve(root, "site-remotion");
const platform = process.platform === "darwin" ? "darwin" : process.platform;
const ffprobe = resolve(
  remotionRoot,
  `node_modules/@remotion/compositor-${platform}-${process.arch}/ffprobe`,
);
const captures: Array<{ feature: string; lang: string; frames: number }> = [];
for (const lang of ["zh", "en"])
  for (const feature of ["ade-herdr", "terminal-git", "remote-db"]) {
    const probe = Bun.spawn(
      [
        ffprobe,
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=nw=1:nk=1",
        resolve(remotionRoot, `public/captures/${feature}-${lang}.webm`),
      ],
      {
        stdout: "pipe",
        stderr: "inherit",
        env: { ...process.env, DYLD_LIBRARY_PATH: resolve(ffprobe, "..") },
      },
    );
    const duration = Number(await new Response(probe.stdout).text());
    if (
      (await probe.exited) !== 0 ||
      !Number.isFinite(duration) ||
      duration < 5
    )
      throw new Error(`Invalid capture ${feature}-${lang}`);
    captures.push({ feature, lang, frames: Math.floor(duration * 30) });
  }
await writeFile(
  resolve(remotionRoot, "src/capture-manifest.json"),
  JSON.stringify(captures, null, 2) + "\n",
);
for (const { feature, lang } of captures) {
  const id = `${feature}-${lang}`;
  if (process.argv.length > 2 && !process.argv.slice(2).includes(id)) continue;
  const render = Bun.spawn(
    [
      "bunx",
      "remotion",
      "render",
      "src/index.ts",
      id,
      resolve(root, `site/assets/${id}.mp4`),
      "--codec=h264",
      "--crf=23",
      "--concurrency=4",
      "--log=error",
    ],
    { cwd: remotionRoot, stdout: "inherit", stderr: "inherit" },
  );
  if ((await render.exited) !== 0) throw new Error(`Render failed: ${id}`);
  console.log(`Rendered ${id}`);
}
