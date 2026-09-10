# Public interactive demo

Run `bun run demo:dev` or build with `bun run demo:build`. The Pages workflow builds this entry into `site/demo/` with relative asset URLs, so it works under a repository subpath.

Before the full test suite on a fresh checkout, run `bun run site:companions` and `bun run demo:build`. The product-page tests verify local links against the generated Pages artifact, including `demo/`; CI runs these build steps before tests.

`main.tsx` installs the in-memory Tauri transport before dynamically importing the page. `Demo.tsx` reuses the production AppShell and stores. No bridge is mounted that restores a real session, starts an agent, or connects to a host. The normal desktop entry never imports this directory.

Supported tours: sample terminal (`help`, `ls`, `git status`, `bun test`, `clear`), file editing and tabs, Git unified/split diffs and staging sample changes, SQL over agents/sessions/workspaces, theme/accent selection, bilingual UI, Space appearance and sidebar controls. Other native operations are unavailable in the demo.

Query parameters: `lang=en|zh-TW`, `theme=light|dark`, `accent=lime|blue|violet|coral|amber`, `scene=terminal|editor|git|database|appearance`. The landing page passes its active language and theme to Demo links. Files and database results reset on reload; the real app's Space appearance preferences remain scoped to the browser origin.
