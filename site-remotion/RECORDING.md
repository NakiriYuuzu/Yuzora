# Current UI media

The published compositions use recordings of `src/demo/Demo.tsx`, which mounts the real AppShell, CodeMirror editor, Git diff, Settings and HERDR terminal page. Native calls are intercepted only by the separate demo entry. Source captures are in `public/captures/`; the existing hand-drawn compositions are no longer registered.

1. Build the preview: `bun run demo:build` from the repository root.
2. Serve `site/` with a static HTTP server. Avoid rebuilding while recording.
3. With the user's browser-recording consent, run `YUZORA_DEMO_URL=http://127.0.0.1:4175/demo/ bun scripts/record-site-demo.ts` from the root.
4. Run `bun scripts/render-site-media.ts` from the root. The script measures every capture, updates the manifest, and renders all six MP4s through Remotion 4.0.509.
5. Run `bun scripts/render-readme-media.ts` from the root to refresh both README languages, their GIF tours, and the Pages posters with matching Remotion frames.

Both scripts accept optional IDs such as `remote-db-zh remote-db-en` to update specific clips. The recording driver uses Playwright CLI and writes real UI posters to `site/assets/`. Playwright's FFmpeg must be installed (`bunx playwright install ffmpeg`). The Remotion package has its own FFmpeg runtime.

The Demo is explicitly sample data: files are editable in memory, the terminal recognizes a small set of example commands, SQL supports SELECT columns / ORDER BY / LIMIT over three sample tables, and native-only actions display an explanatory message. It does not start a shell, connect to hosts, or read the visitor's filesystem. No capture contains a real user workspace.

Pages builds `site/demo/` during deployment; the generated JavaScript bundle is ignored by Git. The workflow also regenerates the landing-page companions from the real SpaceCharacter component and styles.
