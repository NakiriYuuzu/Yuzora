# Current UI media

The published compositions use recordings of the Tauri app's Demo (`src/demo/Demo.tsx` at tag `v0.1.2`), which mounts that app's AppShell, CodeMirror editor, Git diff, Settings and HERDR terminal page. Its source left the repository with the Tauri app; the last build of it is the static copy in `site/demo/`. Source captures are in `public/captures/`; the existing hand-drawn compositions are no longer registered.

1. Serve `site/` with a static HTTP server.
2. With the user's browser-recording consent, run `YUZORA_DEMO_URL=http://127.0.0.1:4175/demo/ bun scripts/record-site-demo.ts` from the root.
3. Run `bun scripts/render-site-media.ts` from the root. The script measures every capture, updates the manifest, and renders all six MP4s through Remotion 4.0.509.
4. Run `bun scripts/render-readme-media.ts` from the root to refresh both README languages, their GIF tours, and the Pages posters with matching Remotion frames.

Both scripts accept optional IDs such as `remote-db-zh remote-db-en` to update specific clips. The recording driver uses Playwright CLI and writes real UI posters to `site/assets/`. Playwright's FFmpeg must be installed (`bunx playwright install ffmpeg`). The Remotion package has its own FFmpeg runtime.

The Demo is explicitly sample data: files are editable in memory, the terminal recognizes a small set of example commands, SQL supports SELECT columns / ORDER BY / LIMIT over three sample tables, and native-only actions display an explanatory message. It does not start a shell, connect to hosts, or read the visitor's filesystem. No capture contains a real user workspace.

`site/demo/` and the landing-page companions (their markup in `site/index.html`, their styles in `site/assets/brand/companions.css`) are committed as they were last generated from the Tauri app; Pages publishes them unchanged.
