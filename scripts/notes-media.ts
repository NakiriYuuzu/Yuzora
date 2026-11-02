import { S3Client } from "bun";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { mkdir, mkdtemp, readdir, rm } from "node:fs/promises";

const root = resolve(import.meta.dir, "..");
const mediaDir = process.env.HERDR_NOTES_MEDIA_DIR || "/var/tmp/herdr-notes-media";
const limit = 1_500_000;
export function version(value: string) {
  if (!/^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$/.test(value)) throw new Error("expected a version such as 0.1.7 (without v)");
  return value;
}
export function mediaUrls(changelog: string, v: string) {
  version(v);
  const sections = changelog.split(/^## /m).slice(1);
  const entries = sections.filter(s => s.startsWith(`[${v}] - `));
  if (entries.length !== 1) throw new Error(`expected exactly one CHANGELOG entry for ${v}`);
  const urls = entries[0].split("\n").flatMap(line => {
    line = line.trim();
    if (!line.startsWith("![")) return [];
    const at = line.indexOf("](");
    if (at < 0 || !line.endsWith(")")) throw new Error("malformed CHANGELOG media");
    return [line.slice(at + 2, -1)];
  });
  return [...new Set(urls)];
}
async function run(args: string[], env = process.env) {
  const p = Bun.spawn(args, { cwd: root, env, stdout: "inherit", stderr: "inherit" });
  if (await p.exited !== 0) throw new Error(`failed: ${args[0]}`);
}
async function check(v: string) {
  const urls = mediaUrls(await Bun.file(join(root, "CHANGELOG.md")).text(), v);
  for (const url of urls) {
    if (!url.startsWith("https://")) throw new Error(`media must use HTTPS: ${url}`);
    if (/\s/.test(url)) throw new Error(`invalid media URL: ${url}`);
    const response = await fetch(url, { method: "HEAD", signal: AbortSignal.timeout(30_000) });
    if (response.status !== 200 || !response.headers.get("content-type")?.toLowerCase().startsWith("image/")) {
      throw new Error(`missing image: ${url} (HTTP ${response.status})`);
    }
    console.log(`ok ${url}`);
  }
  console.log(`checked ${urls.length} media URLs for ${v}`);
}
export async function uploadFile(s3: S3Client, key: string, file: ReturnType<typeof Bun.file>) {
  // Bun signs the PUT; If-None-Match makes refusal atomic, including concurrent uploaders.
  const url = s3.file(key).presign({ method: "PUT", expiresIn: 60, type: "image/webp" });
  let response: Response;
  try {
    response = await fetch(url, { method: "PUT", body: file, headers: { "Content-Type": "image/webp", "Cache-Control": "public, max-age=31536000, immutable", "If-None-Match": "*" }, signal: AbortSignal.timeout(60_000) });
  } catch { throw new Error(`upload failed for ${key}`); }
  if (!response.ok) throw new Error(`upload failed for ${key}: HTTP ${response.status}`);
}
async function upload(v: string) {
  // Parse data, never source credentials as shell code or include them in errors.
  const credentials = await Bun.file(join(homedir(), ".config/herdr-media/env")).text();
  const values = Object.fromEntries(credentials.split("\n").flatMap(line => {
    const match = line.match(/^(?:export\s+)?(R2_MEDIA_ACCESS_KEY_ID|R2_MEDIA_SECRET_ACCESS_KEY)=(.*)$/);
    return match ? [[match[1], match[2].trim().replace(/^(['"])(.*)\1$/, "$2")]] : [];
  }));
  if (!values.R2_MEDIA_ACCESS_KEY_ID || !values.R2_MEDIA_SECRET_ACCESS_KEY) throw new Error("missing media credentials");
  const s3 = new S3Client({ bucket: "herdr-media", endpoint: "https://a5347ea585607d822079d0ee4fd5a8aa.r2.cloudflarestorage.com", region: "auto", accessKeyId: values.R2_MEDIA_ACCESS_KEY_ID, secretAccessKey: values.R2_MEDIA_SECRET_ACCESS_KEY });
  const dir = join(mediaDir, v);
  const names = (await readdir(dir)).filter(n => /^[a-z0-9-]+\.webp$/.test(n)).sort();
  if (!names.length) throw new Error("no WebP clips to upload");
  // Validate the entire set before making the first write.
  for (const name of names) {
    const file = Bun.file(join(dir, name));
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (!file.size || file.size > limit || new TextDecoder().decode(bytes.slice(0, 4)) !== "RIFF" || new TextDecoder().decode(bytes.slice(8, 12)) !== "WEBP") throw new Error(`invalid or oversized WebP: ${name}`);
    if (await s3.file(`${v}/${name}`).exists()) throw new Error(`refusing to overwrite ${v}/${name}`);
  }
  for (const name of names) {
    const key = `${v}/${name}`;
    await uploadFile(s3, key, Bun.file(join(dir, name)));
    console.log(`https://media.herdr.dev/${key}`);
  }
}
async function clip(scene: string, v: string) {
  if (!/^[a-z0-9-]+$/.test(scene)) throw new Error("invalid scene name");
  const script = join(root, "scripts/notes-clips", scene + ".sh");
  if (!await Bun.file(script).exists()) throw new Error(`unknown scene: ${scene}`);
  const name = `notes-clip-${scene}-${process.pid}`;
  const env = { ...process.env, HERDR_APP_SANDBOX: name, HERDR_APP_HOME: `/var/tmp/herdr-app-${name}/home` };
  const frames = await mkdtemp("/var/tmp/herdr-notes-frames-");
  const pointerFile = join(frames, "pointer.json");
  const crop = (await Bun.$`bash ${script} crop`.cwd(root).quiet().text()).trim();
  if (!/^\d+:\d+:\d+:\d+$/.test(crop)) throw new Error("scene crop must be width:height:x:y");
  const pointers: { x: number; y: number; cursor: string }[] = [];
  const sandbox = join(root, "scripts/sandbox");
  let action: ReturnType<typeof Bun.spawn> | undefined;
  try {
    await run(["bash", sandbox, "up"], env);
    await run(["bash", script, "setup"], env);
    action = Bun.spawn(["bash", script, "play"], { cwd: root, env: { ...env, HERDR_NOTES_POINTER_FILE: pointerFile }, stdout: "inherit", stderr: "inherit" });
    const start = performance.now();
    for (let i = 0; i < 60; i++) {
      await Bun.sleep(Math.max(0, start + i * 1000 / 12 - performance.now()));
      await run([join(root, "target/debug/herdr-app"), "shot", join(frames, `${String(i).padStart(3, "0")}.png`)], env);
      pointers.push(await Bun.file(pointerFile).exists() ? await Bun.file(pointerFile).json() : { x: 650, y: 400, cursor: "arrow" });
    }
    if (performance.now() - start > 5500) throw new Error("capture fell behind 12 fps; close other workloads and retry");
    if (await action.exited !== 0) throw new Error("scene failed");
    // Composite after capture so image conversion cannot disturb the frame cadence.
    for (let i = 0; i < pointers.length; i++) {
      const p = pointers[i];
      if (!["arrow", "pointer", "grab"].includes(p.cursor) || !Number.isInteger(p.x) || !Number.isInteger(p.y)) throw new Error("invalid pointer telemetry");
      const frame = join(frames, `${String(i).padStart(3, "0")}.png`);
      const hotspot = p.cursor === "arrow" ? [2, 2] : p.cursor === "pointer" ? [12, 2] : [13, 16];
      const x = Math.max(0, p.x - hotspot[0]), y = Math.max(0, p.y - hotspot[1]);
      await run(["magick", frame, "-background", "none", join(root, "scripts/notes-clips/cursors", p.cursor + ".svg"), "-geometry", `+${x}+${y}`, "-composite", frame]);
    }
    const dir = join(mediaDir, v);
    await mkdir(dir, { recursive: true });
    const out = join(dir, scene + ".webp");
    for (const width of [1200, 1100, 960]) {
      await run(["ffmpeg", "-hide_banner", "-loglevel", "warning", "-y", "-framerate", "12", "-i", join(frames, "%03d.png"), "-vf", `crop=${crop},scale=${width}:-2:flags=lanczos`, "-c:v", "libwebp_anim", "-lossless", "1", "-compression_level", "6", "-loop", "0", out]);
      if (Bun.file(out).size <= limit) { console.log(`${out} (${Bun.file(out).size} bytes)`); return; }
    }
    await rm(out);
    throw new Error("clip exceeds 1.5 MB; simplify the scene");
  } finally {
    if (action && action.exitCode === null) { action.kill(); await action.exited; }
    await run(["bash", sandbox, "down"], env);
    await rm(frames, { recursive: true, force: true });
  }
}
if (import.meta.main) {
  try {
    const [command, arg, optionalVersion] = Bun.argv.slice(2);
    if (command === "check") await check(version(arg));
    else if (command === "upload") await upload(version(arg));
    else if (command === "clip") {
      const cargo = await Bun.file(join(root, "Cargo.toml")).text();
      await clip(arg, version(optionalVersion || cargo.match(/^version = "([^"]+)"/m)![1]));
    } else throw new Error("usage: notes-media.ts clip <scene> [version] | check <version> | upload <version>");
  } catch (error) {
    // S3 errors can contain signed URLs/credential details: don't dump raw SDK errors.
    console.error(error instanceof Error && !("code" in error) ? error.message : "media operation failed");
    process.exitCode = 1;
  }
}
