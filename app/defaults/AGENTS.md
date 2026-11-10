# Herdr app

<!-- Written by the Herdr app on every start. Edits here are overwritten. -->

This folder is **the Herdr app's whole interface, as files**. The Herdr app
is a desktop app for terminals and coding agents, and it redraws itself from
these files while it runs: change a layout, a key or a panel here and the
window follows within a second. Shipped layouts and mods are Herdr's:
change settings for look, or clone before editing structure/code (see Layouts).
When someone asks for something ("a sidebar
of my PRs", "give ctrl+1 back to the shell", "make it warmer", "move the
agents list to the right"), you do it by editing these files or writing a
mod. Nothing in the default interface is special: the spaces sidebar, the
agents list, the tab bar, menus, dialogs and notifications are built
from the same parts you have.

## Two programs

- **`herdr-app`**, the window: spaces, tabs, layouts, panels.
- **herdr-core (`hc`)**, which runs every terminal and agent. They keep
  running when the window closes or updates. You can use `hc` yourself:
  start a terminal, read its screen, send it text, wait for an agent
  (`hc --help`). The Herdr app is one client of it.

`herdr-app` controls this app; `hc` controls herdr-core.

## What you can and can't do

You can: rearrange and restyle the whole window (layouts), rebind any key,
write panels (mods) that list, filter and act on spaces, tabs, agents
and terminals, open dialogs, popups and floating blocks, show a live
terminal inside a block, run any app command, run programs (`exec`) and
fetch URLs, keep state per mod or shared between mods, animate natively.

You can't: use HTML, CSS, a web view, Node APIs or `setTimeout`; draw
outside your block except through overlays; change how a terminal itself
draws text; read or change other mods' code at run time.

## What mods are written in

Mods use **gpui-kit** (its gpui-shell): JavaScript that describes **GPUI**
elements, a native GPU UI (`div()`, flex, children,
`.on_click`). There's no DOM: the script describes, Rust draws. Background:
https://gpui-kit.com. The Herdr app carries its own copy with additions
(`frames`, `loop`), so **the type files here are the truth**:
`gpui-kit.d.ts` for elements and `herdr.d.ts` for the app's data and
commands. They're rewritten on every start to match this version.

## Changing things safely

- Edits apply at once. If a file is invalid, the app keeps the last good
  version and writes why to `<name>.error` (`layout.error`, `keys.error`,
  `settings.error`, `programs.error`). Check after every edit.
- `herdr-app check <mod folder>` loads a mod once and checks its initial
  description and eager native elements, including virtual lists and motion.
  It does not lay out the mod or run deferred rows, nested renders, or later
  async states. Never call a mod done without a passing check; also try it
  in the app.
- **Escape hatch:** `ctrl-alt-shift-escape` always works: it sets the shown
  layout and `keys.json` aside (`*.reset-<time>`) and goes back to the
  default `main` layout. Nothing can rebind it.

## Seeing the window

`herdr-app shot [file.png]` saves the last rendered window frame as a PNG
and prints its absolute path (default: `herdr-shot.png` in your current
folder). Use it when debugging mod UIs: inspect spacing, colors, terminal
images, overlays, popups and floats. It captures the app, not the desktop,
at the window’s physical pixel size, even when covered or minimized. No
screen-recording permission is needed. Herdr must be running; before its
first frame or during GPU recovery it reports an error instead of a PNG.
`HERDR_APP_HOME` selects which running window to ask, as with `status`.

## The files

| File | What it decides | Applied |
|---|---|---|
| `layouts/<name>.json` | the window and its whole look: blocks, theme, fonts, backgrounds, pane frames; one file per layout (`main`, `terminals`, `win98`, any you add) | live |
| `keys.json` | what every key does | live |
| `settings.json` | the person: zoom, sounds, fonts for every layout | live |
| `mods/<name>/` | panels: small JavaScript programs | live (hot reload) |
| `programs.json` | how programs show (name, icon, tile), on top of the built-in table | live |
| `icons/` | the shipped program icons | written by Herdr (put your own elsewhere) |
| `frame/` | the app's own screens (welcome, ...) | rewritten by Herdr on every start: don't edit, it won't stick |
| `view.json` | this screen's selected space/tab/pane, zoom, settled tabs, recent spaces and seen attention | written by Herdr |
| `shared.json`, `data/`, `seen.json`, `installed.json`, `window.json`, `windows.json` | mod state, bookkeeping, where the window was (size, maximized), mods' windows (spots, kept ones) | written by Herdr |

Shared state is saved atomically in the background, with the latest change winning.
If `shared.json` is invalid, Herdr logs a warning and keeps it as
`shared.json.corrupt-<time>` before starting empty.
`herdr-app check` uses empty shared state in memory and leaves this file untouched.

## The model: spaces, tabs, terminals

Every layout shows the same world; a layout may show less, never a
different one. Each machine keeps its spaces, tabs and splits in its core's
`herdr-app` document. Connected screens share that layout and live terminals,
not their focus, zoom or attention. Use commands to change structure; there
is no `spaces.json`. On simultaneous edits the newest accepted document wins
(no merging); structural commands fail while that machine's layout is unavailable.
Mod pane props are stored as given, never machine-qualified. Space order is local
machine first, then machines by name; `space.move` stays inside its machine's block.
In the expanded Spaces sidebar, drag a space to reorder it within its machine;
the highlighted line shows where it will land.

- A **space** is a place to work, on one machine: this one, or another
  from `hc machine ls` (`spaces()[i].machine`). Herdr leaves out machines
  added with `--temp` (an agent's short-lived sandbox): no spaces, not in
  `machines()`, and mods listing machines skip `temporary`. Names work as in Herdr:
  a space nobody named is called after where its first terminal is now
  (its repo, else its folder, `~` at home) and follows it; once named
  (`space.rename`), the name stays; renaming to "" goes back.
  `spaces()[i].named` says which. Where a space is now is its terminals'
  live folders (`panes()[i].cwd`), not the folder it was made in: new
  tabs start where its focused pane is, and the default mods take a
  space's repo from its first terminal.
  If starting in an inherited folder fails, new spaces, tabs and splits retry in
  that machine's home. Explicit invalid folders still fail.
- A **tab** is a piece of work in a space: its terminals and how they're
  split right and down. One nobody named is called by its place (1, 2,
  ...), as in Herdr; `tab.rename` names it ("fix login"). A space
  has one or more; closing the last one closes the space (it stays in
  `recent`).
- A tab can be **settled** (`tab.settle`): put away on this screen, still running;
  `settled` on each tab. The shipped layouts don't use it (they list every
  tab); a mod that does can hide or fold settled tabs. Showing one, or an
  agent in it needing the user, brings it back.
- A **pane** is a terminal on screen, or a **mod** beside them (a markdown
  viewer, a board, a git panel): `pane.split {direction, mod, props}`. A
  mod pane splits, zooms, closes and gets a header like any pane, keeps
  the keyboard when clicked, and is saved with its tab; its `props` tell
  it what to show (a file, a folder). Closing a terminal pane ends its
  terminal, and a program exiting closes its pane, even if Herdr was closed.
  Terminal panes are kept by their machine's core (`keep: true`). A lost
  terminal (core crash, reboot, logout) gets a shell in its last folder,
  with the same address; its original command is never run again. Splits
  and mod panes stay. An exited or killed terminal is closed, not restarted.
  Focus, zoom, recent spaces, settled tabs and seen attention stay on this screen.
- Every terminal is a core terminal. Herdr labels the ones it starts with
  `herdr.space=<id>` and `herdr.tab=<id>`. **A terminal with those
  labels shows up there**, even one you start yourself:
  `hc run --keep --label herdr.space=<id> --label herdr.tab=<id> -- claude`.
  Labels use ids local to that machine; remote ids shown by the app have a
  `<machine>/` prefix (strip it when writing labels on that machine). Adoption
  also enables core keep; `terminal.park` removes the pane and disables keep.
  When a core change is needed, `terminal.keep` / `terminal.park` acknowledge
  with `pending: true`, then apply after the machine replies; failures show a toast.
  Normal quit waits for pending terminal changes and layout writes; remote waits are bounded.
  An id Herdr doesn't know becomes a new space or tab (unnamed: Herdr's names).
  Terminals without the labels (another app's) stay out of Herdr's
  screens, and so do other machines' terminals, unless Herdr started them
  for a space on that machine.
- **Agents** are terminals whose program reports a status (OSC 7501):
  `working`, `blocked`, `idle`, or `done` = finished while the user wasn't
  looking (it becomes `idle` once they look). A space's and a tab's
  status is the most urgent of its agents: blocked > done > working > idle.
- **Sub-agents** are terminals an agent started with `hc run`: the core
  records the starter as `parent` (see `terminals()`). They belong to their
  parent's tab without a pane; the `agents` mod shows them,
  `terminal.peek` opens one, `terminal.keep` gives one a pane.
  Ctrl-click (Cmd-click on macOS) a live terminal address in terminal text to
  peek at it. Bare addresses refer to the machine of the terminal showing them;
  use `machine/address` for another machine. Names and non-live addresses aren't links.
- **Agent navigation order** belongs to the mods. A mod displaying agents calls
  `setAgentOrder(list.map(a => a.address))` on render so `agent.next` /
  `agent.prev` follow its list.

## Layouts: `layouts/<name>.json`

Each file is one whole window. <!-- yuzora(L1): -->`layout.switch` cycles through them (no
key is bound to it by default, and the command palette has no "Next layout":
in Yuzora it would leave for Herdr's own layout)<!-- /yuzora(L1) -->, `layout.switch {name}` picks one; `shared("layout")` is the
shown one's name. Herdr ships `main` ("Herdr": spaces by machine and the
agents list on the left, `workspace-chrome`; tabs as pills with a + menu
that starts a terminal or any installed agent, `new-menu`; panes as cards
with a header; Files and Changes panels on the right, `files` and
`changes`, opened from the tab bar and remembered per space (which ones:
shared `"workspace.panels"`, set by the welcome); a status bar,
`workspace-status`), and as examples `terminals` ("Terminals": tabs in
the title bar, panes as cards, a space picker) and `win98`
("Herdr 98": a whole desktop in one mod, `win98`). Add a layout by
writing a new file. The Files panel supports Windows folder paths with spaces,
parentheses and apostrophes, and preserves Unicode file and folder names
(including Chinese). Directory links expand as folders; their contents load
only when expanded.

**Herdr's own layouts and mods are read-only.** They are replaced whole
on every start. Use settings.json for look (fonts, sizes, theme, colors,
sounds), where the layout leaves them unset. Herdr (main) has no look of
its own, so your settings decide it; Herdr 98 and Terminals bring their own
look. The theme picker overrides any layout. To change a shipped layout's
look beyond that, or its structure/code, clone it and edit the clone:

```sh
herdr-app do layout.clone '{"name":"main","as":"my-layout"}'
herdr-app do mod.clone '{"name":"files","as":"my-files"}'
```

The default clone name is `<name>-copy`; an existing name is refused.
`layout.clone` switches to the clone. In the palette: Duplicate layout
or Duplicate mod. Clones are yours and never updated. They record
`cloned_from: {name, version}` and copy the layout and its clicks as-is,
including its look. A cloned layout keeps its mod references:
clone a mod too and point your layout's `content.mod` to it to change code.
Mod clones rewrite their own `mods/<name>/` asset paths and self-opening
overlay names to the new name.

If you edit a shipped file anyway, the next start moves it to
`<name>-edited` (a mod folder or layout JSON; a number is added if taken),
copies its layout clicks, restores Herdr's version and shows a toast.
No-longer-shipped things go if
untouched, otherwise they stay.

Clicks (sizes, shown/hidden, stack child, floating spots, theme picks,
background, text size, pane header height and border/focus look) live
in `layouts/.state/` and survive restarts. Don't edit those files.
A deliberate layout-file edit beats a click on that field.
`layout.reset {name}` resets a shipped layout and its clicks, keeping its
previous file aside as `<name>.json.old-<time>`.

A layout is a tree of nodes. Four types:

- `block`: shows content. `"content"` is `{ "mod": "<folder>", "props": {...} }`,
  `{ "terminal": "t7" }`, `{ "tab": "active" }` (the shown tab's
  terminals as it arranges them), or `{ "agent": "focused" }` (one agent's
  terminal, full size: the one chosen with `agent.focus` / `agent.next`, else
  the focused pane's agent, else the first agent).
- `row` / `column`: children side by side / stacked.
- `stack`: shows one child (`"active": "<child id>"`), with an optional
  `"header"` (a mod that draws tabs, or `"tabs"` for a plain built-in strip).

Properties any node can have:
- `id` (unique; commands use it), `size`: `{ "px": 240 }` or `{ "weight": 2 }`
  (shares of what fixed siblings leave), `min` / `max` (px), `hidden`.
- Paint: `background` (a color; `{ "gradient": ["#2a6cbd", "#0b2a57"], "angle": 165 }`;
  or `{ "image": "backgrounds/sky.jpg", "fit": "cover" | "contain" }`, a path in
  the home folder), `border` (`{ "width": 1, "color": "border" }`), `radius`,
  `shadow` (`"sm" | "md" | "lg"`), `focus` (`{ "border": {...} }` when the
  focused pane is inside).
- `padding`: space inside the node's edge, px: `16` or `[vertical, horizontal]`.
- row/column: `gap` (px between children), `divider` (`{ "width": 1, "color": "border" }`),
  `resizable` (default true: drag the boundary). This applies to that row/column only.
- `movable` (default true): panes can be dragged by the app's top-center grip within a split tab.
  Unlike `resizable`, it inherits: the nearest explicit ancestor wins, through the
  tab tree and then the layout's tab block and its ancestors. Root `"movable": false`
  opts the layout out. Composition terminal blocks aren't tab panes and never drag.
  Center drop swaps panes; an edge drop places beside it. Escape or dropping outside
  another pane cancels. Zoomed tabs and one-pane tabs don't drag. The grip appears
  on top-edge hover, regardless of the header's position or presence. Only the grip
  starts dragging; header mods don't need drag handlers. `panes()` exposes `movable`.
- `floating`: blocks over this node (below).
- `window_drag: true`: dragging the node moves the window, a double-click
  maximizes it. A mod's own control inside opts out by stopping the press:
  `.on_mouse_down("left", (_e, cx) => cx.stop_propagation())`.
- Root only: `"titlebar": "none"` hides the system title bar so the layout
  draws its own (a bar mod in a `window_drag` block). macOS keeps its window
  buttons over the layout at `"traffic_lights": [x, y]` (px); leave them
  room. Where the system draws no buttons (`window().buttons`, Linux and
  Windows), the bar draws them with `window.minimize`, `window.maximize`, `window.close`.
  Windows' shipped tab bar and system strip use vector window icons, with a
  restore icon when maximized, rather than font glyphs.
- Root only: `"preview": "layouts/<name>.svg"` (or png): the picture the
  settings screen shows for this layout. Draw one when you make a layout;
  without it the person sees a plain frame. New layouts and `themes/*.json`
  show up in settings by themselves.
- Root only: `"terminal_menu": "my-menu"` chooses the mod for terminal
  right-clicks (including Shift+right-click). Omit it for the shipped `menu`;
  an unknown mod logs a warning and falls back to `menu`. Clone a shipped
  layout before changing this field. Other menus are unchanged.
- Colors are theme names (`border`, `accent`, `surface`, ...) or `#rrggbb[aa]`.

A personal terminal menu replaces the whole menu; it doesn't merge items.
Create `mods/my-menu/main.js` and `mod.json` (e.g.
`{"place":"none","overlay":{"width":280,"height":100}}`), then set the layout root's
`"terminal_menu": "my-menu"`. It opens as a pointer popup, default 220×260 px;
its `mod.json` overlay width/height override those defaults. It gets
`props()` = `{tab: "pane", target: {pane, terminal}, overlay}`. Always use
`target.terminal` or `target.pane` in actions, not the focused terminal:
right-clicking a different terminal must act on the one clicked. A terminal
shown outside a tab (a layout terminal block, a window) has no pane:
`target.pane` is its block's id, not in `panes()`, so leave out pane items
there, as the shipped menu does. For example:

```js
import { View, div } from "gpui-kit";
import { props, run, closeOverlay } from "herdr";
export default class MyMenu extends View {
  render() {
    return div().id("split-target").child("Split this pane").on_click(() => {
      const {target, overlay} = props();
      closeOverlay(overlay);
      run("pane.split", {pane: target.pane, direction: "right"});
    });
  }
}
```

### The look: on the layout, cascading

A layout's own look wins; settings.json fills what it leaves unset.
The theme picker overrides both. These go
on any node, and the nearest node up the tree wins, so the root sets the
layout's look and a block can differ:

```json
{ "id": "root", "type": "row", "theme": "graphite", "colors": { "accent": "#a7b4c6" },
  "font": "Geist Mono", "font_size": 15, "ui_font": "Geist",
  "terminal_padding": [10, 6], "terminal_opacity": 1, ... }
```

- `theme`: `graphite`, `ink`, `moss` (dark), `paper` (light), or the name
  of a file in `themes/`; or one for each appearance:
  `"theme": { "light": "paper", "dark": "graphite" }` follows the system's
  light or dark mode (or the person's `appearance`). `app.theme {name}`
  sets the half of the name's lightness. A layout without a theme shows
  settings.json's theme, else graphite or paper. `colors` overrides any theme color by name (see
  `theme()` below), for terminals and panels alike: overrides apply in
  both appearances, so name theme colors (`chrome`, `canvas`) rather than
  writing hex when a layout should work light and dark.
- `themes/<name>.json`: `{ "background", "foreground", "selection",
  "cursor"?, "ansi": [16 colors], "colors"?: {...} }`, all `#rrggbb`; panel
  colors are derived from these, `colors` overrides them. Herdr ships
  `atom-one-dark`. The original unedited Atom One Dark and Win98 theme files
  are upgraded to readable terminal colors; edited theme files are kept.
  A theme named like a layout (`themes/win98.json`) is
  that layout's own: Settings offers it only there.
- `font` / `font_size`: the terminals' (px at zoom 1); `ui_font`: the mods'
  (their text sizes are their own, in px, and follow zoom). Optional: without
  them a layout uses settings.json's, else Herdr's (JetBrains Mono 14, Geist).
  Herdr (main) leaves these unset; Terminals sets text size 15, and Herdr 98
  brings its own fonts.
  JetBrains Mono, Geist Mono and Geist ship with Herdr; any installed font
  works (a terminal font the machine lacks falls back to JetBrains Mono).
  A layout brings its own fonts: `.ttf`/`.otf` files in a mod's `fonts/`
  folder (or the home's `fonts/`) load at start and on the next look change,
  so a layout and its mod folder travel together (Herdr 98's do: its pixel
  fonts are in `mods/win98/fonts/`).
- `pane_scrollbars` in settings.json (default `true`): show terminal scrollback
  with a slim thumb and track. Drag the thumb or click the track to jump there.
  With little or no terminal padding, the hit lane overlays the last 8px;
  terminal columns do not change. Hidden in alternate-screen programs.
- `terminal_opacity` (0..1): below 1, the background behind a terminal
  (a pane frame's, the layout's gradient) shows through its text.
- Switching layouts switches the look. The theme menu changes the shown
  layout's root `theme`.
- Today only the root's look applies; a block's own look is coming.

### Pane frames

The tab block says how each pane of the shown tab is wrapped:

```json
{ "tab": "active",
  "pane": { "gap": 6, "radius": 6, "background": "surface",
            "border": { "width": 1, "color": "border" }, "focus": { "width": 1, "color": "accent" },
            "header": { "content": { "mod": "pane" }, "height": 30, "position": "top", "reveal": "always" },
            "lone": false } }
```

- The look applies to every pane; `focus` replaces the border while the
  pane has the keyboard; `gap` is the space between panes.
- `header`: a mod drawn on each pane, one copy per pane, with
  `props().pane` and `props().terminal`. It reads its live facts from
  `panes()` (title, folder, branch, agent status, focused, zoomed) and its
  buttons run pane commands with that pane (`pane.split`, `pane.zoom`,
  `pane.close`). `position`: `top` or `bottom`; `reveal: "hover"` shows it
  only while the pointer is on the pane, drawn over the terminal so the
  terminal never resizes. The shipped `pane` mod is an example header.
- `lone: true` frames a tab's only pane too (default: only when split).
- Each layout has its own, so one can stay bare while another makes
  every pane a card.

### Floating blocks

```json
"floating": [ { "id": "board", "content": { "mod": "board" }, "edge": "left", "align": "center", "peek": 52 } ]
```

A floating block sits over its node, `edge` (`top`, `right`, `bottom`,
`left`) and `align` along it (`start`, `center`, `end`), 12 px in. It sizes
to its content (render nothing to hide it). People drag it anywhere; on
release it springs to the nearest edge and third, and Herdr saves the new
spot. Clicks and hovers on it never reach what's underneath.

`peek` (side edges only) makes it a drawer: only that many px next to the
edge show until it's hovered, then the rest slides out; it slides back a
moment after the pointer leaves. The mod draws one full layout and never
changes it on hover (that's what keeps it smooth): put what should peek
(status, icons) on the edge side of each row.

Herdr draws the frame around a floating mod (theme background, border,
rounded corners, shadow; accent border while dragged), like a dialog's,
with 5 px above and below. The mod draws only its content, no panel: on a
side, rows can run edge to edge. It reads `placement()`:
`{ edge, align, vertical, hovered, dragging }`. Lay out vertically when
`vertical` (left/right edges), horizontally otherwise; on the right edge,
mirror rows so their edge side is on the right.

The default window:

```json
{ "id": "root", "type": "row", "divider": { "width": 1, "color": "border" },
  "children": [
    { "id": "sidebar", "type": "column", "size": { "px": 240 }, "min": 40, "max": 480,
      "divider": { "width": 1, "color": "border" },
      "children": [
        { "id": "spaces", "type": "block", "content": { "mod": "spaces" } },
        { "id": "agents", "type": "block", "content": { "mod": "agents" } } ] },
    { "id": "work", "type": "column",
      "children": [
        { "id": "tabs", "type": "block", "size": { "px": 30 }, "content": { "mod": "tabs" } },
        { "id": "main", "type": "block", "content": { "tab": "active",
          "pane": { "gap": 6, "radius": 6, "border": { "width": 1, "color": "border" },
                    "focus": { "width": 1, "color": "accent" } } } } ] } ] }
```

Sidebar on the right: swap the two children of `root`. No sidebar: set
`"hidden": true` on `sidebar` (the user can toggle it: `ctrl-shift-b`).
Narrower than 160px, the sidebar mods draw a compact rail; folding resizes
`sidebar` to 44 (76 on macOS, room for the traffic lights). On macOS,
the tabs also leave room for the window buttons when the sidebar is narrower.

### Layouts and mods other people made

People share layouts at herdr.dev/layouts and mods at herdr.dev/mods. Each
is a folder laid out like this home: a layout has `layouts/<name>.json` and
the `mods/<name>/` it needs; a mod is a piece for the person's own window,
so it has only `mods/<name>/` and maybe a `keys.json` with the keys it
expects. Both have a `RECIPE.md` (the brief to build it from scratch) and a
`preview.png`. The files are one build of the recipe. Asked to add one
(`herdr.dev/layouts/<owner>/<name>`, `herdr.dev/mods/<owner>/<name>`, or a
folder on GitHub):

1. Read the recipe and every file before writing anything. Before adding,
   tell the person if a mod reaches the network, runs programs (`exec`,
   `hc`) or reads files outside its own folder.
2. Copy the layout file to `layouts/` and each mod folder to `mods/`. Leave
   the recipe and preview out. Never overwrite: if a name is taken (yours
   or Herdr's), rename the new one: its file or folder, the `mod` names in
   the layout, and the mod's own `mods/<name>/` paths.
3. Fit it to this setup only where its assumptions don't hold (a missing
   program, a name clash) or the person asks. If the files don't work on
   this version, or the person wants it different, build it from the
   recipe instead.
4. A mod's `keys.json`: merge it into the person's. Add its sections and
   keys, and ask before replacing a key they already use.
5. `herdr-app check` each mod; for a layout, then `layout.switch {name}`.
   The person's own layout stays as it was; <!-- yuzora(L1): -->`layout.switch {name}` (or Settings > Mods > Layouts)<!-- /yuzora(L1) -->
   switches back, and deleting the files removes it.

To share one: put that folder in a public GitHub repo (one folder each if
there are several), write its `RECIPE.md`, save a screenshot of it as
`preview.png` and add the repo topic `herdr-layout` for a layout or
`herdr-mod` for a mod. It shows up at herdr.dev/layouts or herdr.dev/mods
within the hour.

A `RECIPE.md` is a brief: another agent with an empty folder builds the
layout or mod from it. Write it from the real files, in this order:

1. `# Name`, one paragraph of what the person sees, `![Name](preview.png)`.
2. **What you see:** the regions top to bottom or left to right, their
   sizes, colors and type; what each one shows.
3. **How it behaves:** every agent status (working, blocked, done, idle)
   and how it looks; what clicks, keys and hovers do; what moves.
4. **Built from:** the layout's node tree and blocks, each mod and its
   `props` roles, the shipped mods it uses, the `herdr` functions and
   commands it calls, any programs it runs, its art.
5. **Watch out:** what tripped the build up and what a fresh build would
   get wrong; limits (sizes, counts); controls that do nothing yet; what
   it assumes about the setup.
6. **Done when:** checks that prove a build works, each one something the
   build really does.

The person and what they see come first; code only in Built from and
Watch out. Exact over vague (`#ab7748`, 29px, `agent.focus`), and nothing
the build doesn't do.

## Keys: `keys.json`

On upgrade, an untouched keys file from an earlier release gets the current
shortcuts. Any edit, even whitespace, keeps the entire file as yours; new default
shortcuts must then be added by hand.

```json
{
  "global":   { "ctrl-shift-t": "tab.new", "ctrl-shift-o": "pane.split:right", "alt-1": "space.select:1" },
  "terminal": { "ctrl-shift-c": "terminal.copy", "ctrl-left": "send:\u001bb", "ctrl-1": "terminal" },
  "layout:terminals": { "ctrl-shift-pagedown": "space.next", "ctrl-shift-pageup": "space.prev" },
  "mod:editor": { "ctrl-s": "editor.save" }
}
```

- Sections: `global`, `layout:<name>` (while `layouts/<name>.json` shows),
  `terminal` (a terminal has focus), `mod:<name>`. The more specific section
  wins: `layout:terminals` over `global`, `terminal` over both.
- An action is a command (`name`, or `name:value`: the value fills the
  command's first argument), `send:<text>` (types into the terminal),
  `<mod>.<command>` (delivered to that mod's `nextCommand()`), or `terminal`
  (the program gets the key).
- Named, typed command arguments use an object instead of a string:
  `"ctrl-shift-r": {"command": "layout.resize", "args": {"id": "sidebar", "px": 240}}`.
  `command` names an app command; `args` is an object (optional, defaults to `{}`).
  It can pass several arguments, numbers, booleans, arrays or nested objects.
  String actions keep their literal first-argument shorthand, including numeric
  indexes: `space.new:{work}` names a space `{work}`, and `space.new:{}` names it `{}`.
  Invalid binding objects or rejected arguments show an error toast when pressed;
  the key does not go to the terminal program.
- Plain ctrl+letters go to the program unless bound. Programs rely on
  ctrl+c, ctrl+d, ctrl+l, ctrl+r, ctrl+w, ctrl+a, ctrl+e, ctrl+u, ctrl+k,
  ctrl+z and ctrl+[ (Escape); ctrl+alt is AltGr on Windows; option+digits
  and letters type characters on many Mac keyboards (`[`, `@`). Don't pick
  these on your own. If the person asks for one, bind it and tell them what
  it takes from typing.
- Windows AltGr and dead-key compositions type text in terminals. macOS
  dead keys compose accents through native text input; Option keeps its
  keyboard-layout characters rather than acting as Alt.
- On macOS, shifted punctuation shortcuts accept both spellings from the
  current layout: `cmd-shift-]` and `cmd-}` match the same US-layout keypress.
  Existing `keys.json` files don't need to change.
- Terminal keys honor the program's negotiated Kitty mode, including on Windows.
- Windows terminal Escape stays separate from the following key.
- Search: `ctrl-shift-f` (Cmd-F on macOS) opens a find bar inside the focused terminal;
  Enter/Shift-Enter moves through matches and Escape closes it while the find field
  has focus. Clicking the terminal puts typing, including Escape, back in the program;
  the bar and its matches stay.
  Each terminal keeps its own query, and reopening selects it so typing replaces it. Search covers the screen and scrollback without replacing text selection.

## Commands

The one way to change anything; keys, mods (`run(name, args)`), menus and
you use the same list. `herdr-app commands` prints every one with its arguments
(which are required) and what it does. Unknown argument names are errors and list
valid names; they never fall back to the current target. For example, closing a
space uses `herdr-app do space.close --id <id> --confirmed true`, not `--space`.
Target IDs (`id`, `pane`, `space`, `terminal`) must be strings. Other non-null
types are rejected before a current-target default applies; null means omitted.
The main ones:

- Spaces: `space.new {name?, folder?, tab?, argv?, task?, machine?}`
  (machine: another machine from `hc machine ls`; omitted: the focused space's
  machine for keys and clicks, this computer for scripts; `""`: this computer.
  The space lives there, its
  terminals start in that machine's home unless folder, and its tabs and
  splits stay there; `spaces()` gives each space's `machine`, null here),
  `space.close {id?, confirmed?}`, `space.rename {id?, name}` ("": Herdr's name again),
  `space.move {id, index}`, `space.select {index | id}`, `space.next`, `space.prev`.
  With no `argv`, the target core chooses that machine's default shell (PowerShell 7 when available on Windows).
- Tabs: `tab.new {name?, space?, folder?, argv?, task?}` (argv: an
  agent to run, task: its first message). Agents run in your interactive
  shell (including fish), which stays open after the agent exits. A terminal
  that exits with a nonzero code within two seconds of launch shows an error
  toast with its exit code and last output lines. Or `tab.new {name?, space?, mod,
  props?}` for a tab whose only pane is a mod (a file, a board; split a shell
  in beside it like any tab), `tab.close {id?, confirmed?}` (closing the last tab
  closes its space and asks first unless confirmed),
  `tab.rename {id?, name}` ("": its number again), `tab.move {id, index}`,
  `tab.select {index | id}`, `tab.next`, `tab.prev`,
  `tab.settle {id?}`, `tab.unsettle {id?}` (index, next and prev
  skip settled tabs).
- Panes: `pane.split {direction: "right" | "down", pane?, argv? | mod?, props?}` (a new
  terminal, or a mod pane), `pane.close {pane?}`,
  `pane.zoom {pane?}`, `pane.focus {pane | terminal}`, `pane.go {direction}`,
  `pane.swap {pane?, direction | target}`.
  `pane.go` prefers the most recently focused pane along the nearest edge in that
  direction; unvisited panes fall back to overlap, then top/left layout order.
  `pane.go` also reaches visible mods whose `mod.json` says `"focusable": true`, and
  starts from that mod while it holds the keyboard.
  `pane.swap {direction}` uses geometry only. Focus history lasts until the window closes.
  `pane.swap {target}` trades the places of `pane` (default: the focused one) and
  `target`, another pane of the same tab; give `direction` or `target`, not both.
  A swap moves panes, not focus: the focused pane stays focused in its new place.
  `pane.focus {terminal}` prefers the first visible layout terminal block showing that
  address, then its model pane. Clicking a terminal block focuses it too.
  While a layout terminal block is focused, implicit split/close/zoom/go/swap and
  `terminal.show` fail, as do `terminal.open`/`terminal.keep` with place right/down.
  They never act on the terminal's source pane. An explicit `pane` still targets that
  model pane; composition edits belong to the mod (edit its layout instead).
- Terminals: `terminal.open {argv?, folder?, place?, name?}` (`folder` sets the
  starting directory for tabs and right/down splits, with or without `argv`), `terminal.kill {terminal}`,
  `terminal.keep {terminal, place?}`, `terminal.park {terminal}`,
  `terminal.peek {terminal}` (peek over the tab, with keep-as-tab and close),
  `terminal.show {terminal?, pane?}`, `terminal.search` (screen and scrollback;
  find bar state stays with that terminal), `terminal.copy`, `terminal.paste` (copied
  files, such as Finder's or a clipboard manager's image, as if dropped; an image
  alone as `terminal.paste_image` pastes it), `terminal.paste_image` (Ctrl+V: an
  image, into a terminal on another machine: copied there, its path pasted; on
  Windows, clipboard text is pasted; otherwise Ctrl+V goes to the program, which
  reads the clipboard itself). TIFF and BMP clipboard
  images are converted to PNG before sending,
  `terminal.right_click {to: "program" | "menu", terminal?}` (where right-clicks go
  when the program takes the mouse; kept as its label `herdr.right-click=program`;
  Shift+right-click is always Herdr's menu),
  `terminal.scroll {lines | pages | to: "top" | "bottom", terminal?}` (moves Herdr's view of the
  history, not the program: negative lines or pages go up, toward history; pages may be
  fractional, one page is the terminal's rows; on the alternate screen, such as vim or less,
  there is no history, so nothing moves),
  `terminal.pin {terminal, pinned}` (pins an agent to the top of the agent lists;
  kept on the terminal as `herdr.pinned`, so every Herdr client shows it, and
  gone when the terminal ends; read it as `agents()` `pinned`). Copy, paste, paste-image and implicit
  right-click targeting follow keyboard focus, including peek and floating terminals;
  with no terminal holding keyboard focus, they use the focused pane or block.
  Dragging a text selection past the terminal's top or bottom scrolls while held;
  releasing stops it. Programs that track the mouse still get their mouse events
  (hold Shift to select text instead). A drag stays with the terminal where it
  started, including its release outside the pane; other panes don't receive it.
  Terminals display Kitty graphics, including Unicode placeholder images used by
  programs such as yazi; partial images and scrolling follow the placeholder cells.
- Agents: `agent.next`, `agent.prev`, `agent.focus {terminal}`.
- Layout: `layout.switch {name?}`, `layout.toggle {id}`, `layout.show {id}`, `layout.hide {id}`,
  `layout.resize {id, px | weight}`, `layout.select {stack, child}`, `layout.set {layout}`
  (personal tree edits; on shipped layouts only click/look fields change),
  `layout.reset {name?}` (reset a shipped layout and its clicks),
  `layout.clone {name, as?}` (duplicate, then switch; default `<name>-copy`).
- App: `mod.clone {name, as?}` (duplicate; default `<name>-copy`), `mod.open {name}` (again: closes it), `overlay.close {id?}`, `app.help` (again: closes it), `app.palette` (again: closes it), `app.settings {section?}`
  (sections: `look`, `machines`, `alerts`, `updates`, `privacy`; Machines lists, updates, renames and removes ssh machines; settings edit the same files you do).
  `app.check_updates` (only a successful check reports Up to date; a failed check clears the checked time and can be retried), `app.update_core` (a newer herdr-core of this generation, terminals kept;
  never one a package manager owns), `app.welcome`, `app.whats_new`, `app.feedback {message?, from?}` (from `"person"` or `"agent"`; opens a review window, where the person can paste up to 3 screenshots; never sends until the person clicks Send), `app.reset`,
  `app.theme {name}`, `app.zoom_in`, `app.zoom_out`, `app.zoom_reset`,
  `app.update` (restart into a downloaded update).
  Add machine lists all named ssh-config hosts in a scrollable list. Machines
  that need a password, a new host key or herdr-core installed show what to run
  in your own terminal; the picker updates once it is added. Other machines are
  added quietly. If a remote machine was rebuilt
  or replaced and a different core answers there, its sidebar offers Connect:
  explicitly forget the old identity and pair again with the same name and target.
  Other unreachable machines don't offer this.
- Window: `window.minimize`, `window.maximize` (or back), `window.close`.
- macOS: `app.quit`, `app.hide`, `app.hide_others`, `app.show_all`.

When the user runs a command without a required name (a key, a click), Herdr
asks them. **You never get a dialog**: pass every required argument, or the
command says what's missing.

From a shell:

```sh
herdr-app do tab.new --name "fix login" --argv '["claude"]' --task "look at the login bug"
herdr-app do pane.split:right
herdr-app ls          # spaces, their tabs and terminals (sub-agents under their parent)
herdr-app status      # what's on screen: layout, focus, overlays, zoom, errors from files and mods
herdr-app commands    # every command and its arguments
```

`herdr-app do` prints what the command made (`{ "space", "tab",
"terminal" }`, `{ "pane", "terminal" }`) or `{ "error": "..." }` and exits 1.

## The person: `settings.json`

What's about the user, never a look:

```json
{ "zoom": 1, "sounds": { "enabled": true } }
```

- `zoom` scales the whole app: terminals, mods, the layout's px sizes
  (0.5 to 3; the zoom keys change it). Write sizes as designed at zoom 1.
- `sounds`: `{ "enabled": true, "done": "<file>", "blocked": "<file>" }`.
  When an agent needs the person, or finishes while they aren't looking at
  it, Herdr plays a sound and (unless they're looking) posts a toast that
  goes there when clicked. `done` / `blocked` replace the bundled MP3s
  (path absolute or relative to `~/.config/herdr-app`). Changes apply live,
  including the Play buttons in Settings > Alerts.
- `core`: the herdr-core binary to use, when it isn't where Herdr looks
  (PATH, the login shell's PATH, `~/.local/bin`, Homebrew, mise, Nix,
  cargo). Read at start.
- `font`, `font_size`, `ui_font`: used where the layout doesn't set its
  own. Herdr (main) leaves these unset. Bigger panels: `zoom`.
- `font_ligatures`: programming ligatures are off by default; set `true` to
  enable them in terminal fonts such as JetBrains Mono and Fira Code. The cursor
  and background changes (including selections) break ligatures so individual cells stay clear.
- `theme`: a theme name or `{light, dark}`; used where the layout sets none.
  The theme picker overrides any layout and stays on that layout.
- `colors`: theme color overrides (`{"accent":"#ff8800"}`), with the same
  precedence as fonts.
- `appearance`: `"light"` or `"dark"` always; absent, the system's. A layout
  with a light and a dark theme shows the matching one.
- Updates: an installed app downloads them by itself and offers Restart and
  What changed. If the app crashes or is killed with a verified newer update
  waiting, the next launch installs it before opening a window. The first
  start after an app update also updates herdr-core
  to the latest release of the same generation, keeping terminals running.
  Settings → Updates keeps app and core actions together;
  its What's new button opens the waiting update's release range, or this
  version's notes, in the What's new window. Reading never clears the waiting-update dot.
  Rare spotlight releases open What’s new after restart, once; otherwise a
  quiet Updated pill lasts for that launch. A build from source never updates.
  Curated notes come from the bundled root `CHANGELOG.md`; remote notes and
  media are optional. `update-notes.json` records the last successful app
  version and presented spotlights; `update-notes-cache.md` caches remote
  notes; `update-notes-media/` caches HTTPS images (including animated WebP clips),
  referenced below its item in the changelog as `![label](https://...)`.
  Each version uses `### New` with `- **Headline.** detail` highlights and
  `### Fixed` with short bullet items. Unsectioned bullets are plain items.
  Notes expose `items` with `headline`, `detail`, `fixed`, and cached `media`
  objects (`path`, pixel `width`, pixel `height`). Highlights use matching cards:
  edge-to-edge media at its original aspect ratio, then headline and detail.
  For local release-media previews only, launch with
  `HERDR_NOTES_MEDIA_DIR=/var/tmp/herdr-notes-media`: URLs under
  `https://media.herdr.dev/<version>/<scene>.webp` use that directory's
  `<version>/<scene>.webp` instead of downloading. Without the variable,
  media uses the normal HTTPS cache.
  Mods use `updates().notes` and `presentUpdateNotes([version])` only
  for spotlight entries actually shown expanded, not collapsed headlines.
  macOS releases require Apple Silicon; Intel Macs must build from source.
  Keep signed macOS bundles unchanged: don't edit their Info.plist or
  re-sign them. On launch, an installed macOS bundle creates or repairs its
  own `~/.local/bin/herdr-app` symlink, leaving personal files and other
  apps' links alone. Disk-image and translocated launches don't create it.
  Finder launches load the login shell's PATH, with the usual install
  folders as a fallback.
- `option_as_alt`: macOS only, `"auto"` (default), `"on"`, `"off"`, `"left"`, or `"right"`.
  Available in Settings → Keyboard.
  `"auto"` sends Option as Alt on US, US International, ABC and ABC-India;
  other layouts keep native Option text and dead keys. `"on"` enables terminal
  Alt shortcuts (including Option+E without composition); `"off"` keeps native
  Option input on every layout. `"left"` / `"right"` makes only that Option
  key act as Alt; the other types characters. Changes apply immediately, including layout
  switches. Linux and Windows are unchanged.
- `minimum_contrast`: terminal text contrast ratio, `1` (default) leaves
  program colors unchanged. Set it to `1.6` to enable the previous readability
  adjustment, or another number from `1` to `21`. Text below that ratio is
  replaced with near-white or near-black, whichever contrasts more with its
  background; this is a threshold, not a guaranteed resulting ratio.
  Box/block, Powerline and sprite glyph colors are never adjusted.
  Changes apply immediately, including text already on screen.
- `clipboard_write`: `false` blocks terminal programs from writing the local
  clipboard (default `true`, including remote terminals attached in this window).
  OSC 52 accepts base64 UTF-8 text up to 1 MiB: `c` writes the system clipboard,
  `p` writes Linux primary selection. `s` is ignored; cut buffers `0`–`7`
  are treated as the system clipboard.
  Clipboard reads (`?`) are never allowed.
- `pane_scrollbars`: `false` hides the scroll position thumb at a terminal
  pane's right edge (default `true`). It shows only when the pane has
  scrollback — never on the alternate screen (editors, pagers) — and
  drags to move through it.
- `telemetry`: `false` stops anonymous usage data and deletes what wasn't
  sent (on when absent; Settings → Privacy shows each report word for
  word, and `telemetry()` in mods). Once a day the app sends counts:
  version, OS, which known agents ran, how many spaces, tabs and
  terminals, which layouts, shipped mods and commands were used. Never
  terminal content, names of files, folders, spaces or the person's own
  mods; nothing from herdr-core. `DO_NOT_TRACK=1` or `HERDR_TELEMETRY=0`
  turns it off for one run. A build from source never sends.

## Programs: `programs.json`

Every pane, agent and terminal carries `program`: its name, icon and tile,
from one table, so every mod shows the same, and the app's own toasts
("Kiro finished") use the same names. An agent's own report wins;
else the program the terminal runs (an interpreter shows what it runs:
`node .../vite` is Vite, `npx vitest` is Vitest); shells are `shell`,
anything unlisted is `program` with a color of its own. The built-in table
has 120 programs (agents, editors, runtimes, tools, databases, infra);
`programs.json` here adds to it or changes entries field by field:

```json
{ "programs": {
    "lazygit": { "name": "Git", "tile": "#f05032" },
    "mytool": { "name": "My Tool", "icon": "my-icons/mytool.svg", "tint": true,
                "tile": "#334155", "color": "#ffffff", "match": ["mytool-dev"] } },
  "shells": ["xonsh"], "interpreters": ["pipx"] }
```

- `icon`: a path in the home folder. `tint: true` for a one-color glyph
  (drawn in `color` on a tile, or the text color), false for a picture.
- `match`: other process names (or agents' `app` names) for the entry.
- `run`: how to start it (`["cursor-agent"]`), for agents people start
  from a menu: `knownAgents()` lists the entries with one, and whether this
  machine (or another) has them installed. For example, override
  `"claude": { "run": ["my-claude"] }` or add
  `"claude-alt": { "name": "Claude (alt)", "icon": "icons/claude.svg", "run": ["my-claude-alt"] }`
  inside `programs`. Use programs or shell functions, not aliases (the launcher
  does not expand them). On macOS the menu launcher loads login profiles before
  running the agent and leaves an interactive login shell when it ends.
  Codex starts as `codex --no-daemon`: a shared Codex daemon would make later
  sessions use the first terminal's identity and stop with it.
  On remote Unix machines, both the installed-agent check and launcher load
  that user's login/interactive shell for sh, bash, zsh, dash, ksh and fish, so
  its PATH includes shell-configured tools. Other shells keep the sh launcher.
  Local Linux launches keep interactive non-login shells. Local and remote checks
  include functions in bash and zsh; fish checks find executables only. Installation
  checks are cached for up to a minute; changing the commands checks again.
  Invalid JSON is reported in `programs.error` (a valid file has no error).
- It follows what runs in the terminal now (lazygit, nvim), else what it
  started with.

## Mods: `mods/<name>/`

```
~/.config/herdr-app/mods/<name>/
  main.js       the mod (required)
  mod.json    where it first appears: { "place": "right" | "left" | "top" | "bottom" | "none", "size": 320 }
  *.js, *.svg   more modules ("./ui.js"), its own icons
```

- `"focusable": true` in `mod.json` lets `pane.go` give a shown copy the keyboard.
  It gets `"focus"` from `nextCommand()`: focus your own handle then
  (`this.handle.focus()`) and handle keys with `on_key_down`. The copy gets `"focus"`
  again after a popup over it closes. Leave with `run("pane.go", {direction})`.
  Without this opt-in, navigation and clicks stay unchanged.

- The first time Herdr sees a folder, it docks one copy per `mod.json` into
  the shown layout (`"none"`: only where a layout or another mod puts it).
  After that, the layouts decide. Saving any file reloads every copy in place
  (not files the app writes: `jsconfig.json`, `.d.ts`, hidden ones,
  `node_modules`). The old build's tasks and timers stop: a loop ends at its
  next `cx` use. A broken save keeps the old version running and shows in
  `herdr-app status`; its top-level code has already run, so keep side effects
  on `globalThis` out of the module's top level.
- A copy starts the first time it shows, then stays alive while its block is in
  the layout, shown or not (a hidden column, the other child of a stack, a pane
  in another tab or behind a zoom): switching back finds it as it was. Its
  timers keep running while hidden, so poll gently. Another layout starts its
  mods fresh (pane mods belong to their tab and stay); a block removed or given
  another mod or props starts over. `<mod>.<command>` reaches shown copies only.
- Images and SVGs are referenced from `~/.config/herdr-app`:
  `image("mods/<name>/logo.svg")` (full color), `svg("mods/<name>/icon.svg")`
  (tinted with the text color). Saving an asset under `mods/` updates every
  mod showing it, including images that were missing or invalid on first load.
- The defaults are mods too, read them as examples: `spaces`, `agents`,
  `tabs` (the tab bar), `menu`, `prompt`, `confirm`, `help`, `toast`,
  Terminals' `picker`, `peek`, and `pane` (an
  example pane header, not used by the default layouts), and `worktree`
  (git worktrees as spaces: the space menu's and the + menu's New worktree,
  Open worktree (the repo's checkouts, filtered; one already shown is gone
  to) and Remove worktree open it as a dialog with props `{space, folder?,
  action: "new" | "open" | "remove"}` (the repo: folder, else where the
  space's first terminal is), or with `{branch, tab?, argv?}` to make one at once with
  that first tab; checkouts go to
  `~/.herdr/worktrees/<repo>/<branch>` on the space's machine, a constant
  at its top; the
  `spaces` sidebar nests worktree spaces under their repo's and shows each
  space's repo, branch, ahead/behind and uncommitted changes). Shipped mods
  are Herdr's; duplicate with `mod.clone` before editing.

### Check before you finish

```sh
herdr-app check ~/.config/herdr-app/mods/<name>
```

Loads the mod, renders it once, exits nonzero with the exact error. Run it
after every change. Never call a mod done without a passing check.

### A mod

```js
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, spaces, theme, run } from "herdr";

export default class Branches extends View {
  init(_props, cx) {
    this.branch = {};
    cx.timer.every(3000, (cx) => this.refresh(cx));
    this.refresh(cx);
  }

  refresh(cx) {
    cx.spawn(async (cx) => {
      for (const p of spaces()) {
        // on: a space on another machine runs git there.
        const r = await exec("git", ["branch", "--show-current"], { cwd: p.folder, on: p.machine });
        this.branch[p.id] = r.stdout.trim();
      }
      cx.notify();
    });
  }

  render(cx) {
    const t = theme();
    return v_flex().size_full().bg(t.surface).p(10).gap(4).text_size(13).children(
      spaces().map((p) =>
        h_flex().id(`b-${p.id}`).gap(8).hover((s) => s.bg(t.raised))
          .on_click(() => run("space.select", { id: p.id }))
          .child(div().text_color(p.active ? t.foreground : t.muted).child(p.name))
          .child(div().text_color(t.dim).child(this.branch[p.id] ?? ""))));
  }
}
```

Rules:
- `main.js` default-exports a class extending `View`. `init(props, cx)` runs
  once; `render(cx)` describes the UI and runs again after `cx.notify()` and
  whenever Herdr's live data changes (spaces, agents, terminals, focus,
  theme, shared state). Don't poll those.
- Module code runs once per source version and is shared by copies, even after all
  copies close. Keep instance state and host calls in constructor/init, not module
  scope. Each copy still gets its own instance, tasks and localStorage.
- **Never keep a `cx` and use it later.** Every callback gets its own:
  `on_click((event, cx) => …)`, `cx.spawn(async (cx) => …)`,
  `cx.timer.every(ms, (cx) => …)`.
- No `setTimeout`/`setInterval` (use `cx.timer`), no top-level `await` (use
  `cx.spawn` in `init`). Anything slow goes through `exec` or `fetch`.
- Size: put `.id("root").on_size_change((size, cx) => { this.size = size; cx.notify(); })`
  on the root `div()`, `h_flex()` or `v_flex()`. It reports `{width, height}`
  in logical pixels after the first layout and whenever the size changes,
  even with the pointer elsewhere. It does not change `props()`.
- Cleanup: an optional `async close(cx)` on the exported `View` gets one final
  call when its pane, tab, layout-owned slot or overlay goes, and on app quit.
  It is not called for a hidden copy or an inactive tab. Start cleanup inside
  `close()`; don't await work started earlier. Await cleanup directly,
  for example `async close(cx) { await exec("hc", ["kill", this.browser]); }`.
  Closing stays responsive: cleanup has at most 250 ms (shared across copies
  on quit), then unfinished tasks are cancelled. On quit, use `exec(...)` and
  `run("terminal.kill", { terminal: address })`: those go out before the event
  loop stops; other commands may not run. An `exec` already started keeps
  running; don't start new long-lived work in `close`. This is best-effort,
  not a guarantee after a crash or forced termination.
- Use `theme()` colors so the mod follows the user's theme.
- **Animate natively, never with a timer.** Every redraw of a mod redraws
  the whole window, so `cx.timer.every(90, () => cx.notify())` for a
  spinner costs real battery. Use `.frames(["⠋", "⠙", …], 90)` (shows
  each string in turn as the element's text), `svg(arc).spin(900)` (turns
  an icon once every 900 ms: a ring spinner, e.g.
  `mods/workspace-chrome/spinner.svg`), or `.loop("opacity", { from:
  0.4, to: 1, period: 1200 })` (also width, height, left, top), and
  `.transition(...)` for changes: they run without your script and stop
  when not shown. Timers are for data (a clock, a fetch), and only as often
  as what they show changes.

### UI

Chained builders with GPUI's snake_case names: `div()`, `h_flex()`,
`v_flex()`; `.size_full()`, `.flex_1()`, `.gap(n)`, `.p(n)`, `.px(n)`,
`.w(n)`, `.h(n)`, `.items_center()`, `.justify_between()`,
`.overflow_y_scrollbar()`; `.child("text")`, `.text_size(n)`, `.font_semibold()`,
`.truncate()`, `.text_color(c)`; `.bg(c)`, `.border(1)`, `.border_color(c)`,
`.rounded(n)`; `.id("unique")` then `.on_click(...)`, `.on_mouse_down("right", ...)`,
`.hover((s) => s.bg(c))`, `.on_scroll_wheel(...)`; keyboard: `this.h = cx.focus_handle()`
in init, `.track_focus(this.h).on_key_down((e, cx) => …)`, `this.h.focus()`.
`.on_drop((event, cx) => …)` on a plain layout element receives external files as
`event.paths` (absolute paths). The feedback form accepts dropped images with
the same limits, previews and removal as picked images.
`.on_hover((hovered, cx) => …)` reports pointer entry and exit on buttons
and other interactive controls too.
`.when(cond, (el) => …)`, `.children([...])`. Controls from `gpui-base`
(`Button`, `Checkbox`, `Switch`, `InputState` + `Input`) are unstyled: style
them. Text: `TextView.markdown(id, text)` renders markdown; `EditorState.new({
language, value, readonly, font_family: settings().font_family, syntax: {
keyword: theme().ansi5, ... } })` + `Editor.new(state)` is a code view (readonly)
or editor with syntax highlighting, line numbers and folding. A viewer opens
beside the shell as a pane: `pane.split {direction, mod, props: { file }}`.
The full surface is in `gpui-kit.d.ts` next to this file.

### The `herdr` module

All of it is in `herdr.d.ts`. In short:

- Live data: `spaces()` (each with its tabs, status, name, named, folder,
  machine, branch), `machines()` (name, null for this computer; layout:
  `loading`, `ready`, or `unavailable` on this connection; loading ends on
  the first read even if it fails, and offline machines are unavailable),
  `agents()` (status, app, space, tab, pinned, focused; "done" turns "idle"
  once someone looks at it on any Herdr client), `terminals()`
  (every core terminal), `focused()`, `layout()`, `theme()` (every color by
  name: background (the terminals', cards), canvas (behind cards), chrome
  (sidebars, bars), surface, popover, raised, hover, selected, border,
  input (a stronger border), foreground, muted, dim, accent,
  accent_foreground, solid and solid_foreground (a primary button),
  selection, working, blocked, done, idle, unknown, ansi0..15; light and
  dark themes name the same ones), `settings()` (with `dark`: the
  appearance now), `keys()`, `commands()` (every app
  command, its arguments and what it does), `layouts()` (the layout names).
  `focused().agent` is the agent an agent block shows.
  `focused().space`/`tab` describe the current window context. With a layout terminal
  block focused, `pane` and `shown` are null; `terminal` and `cwd` describe that block,
  not its source pane. A terminal in a window (`openOverlay` with a `layout`) that has
  the keyboard is reported the same way; pane commands without a `pane` still act on
  the focused pane under the window. Check `focused()?.pane` before offering pane controls.
  `panes()` still lists only model panes; none is focused while a terminal block is.
  For composition navigation: `run("pane.focus", {terminal: nextAddress})`.
  Terminal copy/paste and implicit right-click routing follow the focused block.
- Floating: `placement()` (null unless this copy floats).
- Act: `run(command, args)`.
- Overlays: `openOverlay({ mod, props, kind: "dialog" | "popup" | "window", width, height, at, frame })`
  returns an id; the mod gets `props().overlay` and closes itself with
  `closeOverlay(id)`; Esc closes the top dialog or popup. `frame: "none"`
  drops Herdr's border, corners and shadow so the mod draws its own.
  `notify({ title, body, level, icon, timeout })` posts a notification, drawn by the `toast` mod.
- Windows (`kind: "window"`): stay open until closed, over the layout and
  under dialogs; a press the mod doesn't stop drags it (stop it on your
  controls, like a title bar's buttons), it stays where dropped, and the one
  pressed last is on top. Sized by content unless `width`/`height`; at `at`
  or centered. `key: "name"` remembers its spot (opening that key again
  replaces it); `keep: true` with a key reopens it on the next start. A
  layout switch closes windows; kept ones come back with their layout.
  A window can hold a layout, terminals too: a terminal keeps its presses
  (selecting text), so give it a mod block to drag by, a title bar:
  `openOverlay({ kind: "window", key: "t7", width: 600, height: 400, layout: { type: "column", children: [{ type: "block", size: { px: 28 }, content: { mod: "my-titlebar" } }, { type: "block", content: { terminal: "t7" } }] } })`.
- Machine: `exec(cmd, args, { cwd, on, timeout })` (the user's real environment; use this,
  not `process.run`; `on: space.machine` runs there, omitted means local).
  `timeout` is milliseconds, defaults to 30,000 and is capped at 600,000 (10 minutes),
  locally and remotely. It rejects on timeout, if the program cannot start or the machine cannot be reached.
  `hc(args)` (`hc read <t>` gives a terminal's text),
  `home()`, `appHome()`, `playSound("done" | "blocked" | file)`.
  `knownAgents({ on })`: the agents people can start (programs.json's
  `run`), most picked first, each `installed` or not there (asked of the
  shell agents start in; a remote Unix machine's login/interactive shell when supported);
  start one with `argv: agent.run`.
  `pickFolder({ prompt })`: the system's folder picker (here only),
  resolves with the folder or null.
- Files: `readTextFile(path, { on })` (here, or on that machine),
  `writeTextFile(path, text, expected)` (here only), and
  `watchFolder(dir)` (here only; poll a remote space's folder): resolves with the changed paths the next time anything
  under `dir` changes. On Linux, Git-ignored directories and Git objects are skipped,
  and trees above 4096 non-ignored directories reject the watch; use a timer fallback.
  <!-- yuzora(perf): -->
  macOS and Windows watch recursively, but skip paths inside a directory that
  `dir`'s own `.gitignore` or `dir/.git/info/exclude` ignores (`target/`, `node_modules/`;
  watching `repo/src` doesn't read `repo/.gitignore`): a mod that needs those changes polls
  with a timer. An ignored file itself (`.env`) is still reported, and so is everything in `.git`.
  <!-- /yuzora(perf) -->
  A returned root path means refresh everything (the pending paths are bounded).
  Files also refreshes on its 10-second timer:
  `cx.spawn(async (cx) => { for (;;) { await watchFolder(dir); this.reload(); cx.notify(); } })`.
- `notify({ ..., action: { name, args } })`: the toast runs the command when clicked.
- This copy: `props()`, `shared(key)` / `setShared(key, value)` (every mod
  sees it; setting re-renders all), `nextCommand()` (key commands for this
  mod: `cx.spawn(async (cx) => { for (;;) { const c = await nextCommand(); …; cx.notify(); } })`).

Mods are trusted code: commands and network access (including `fetch`) are unrestricted.
Other built-ins: `localStorage` (per mod copy), `console.log`, `fetch`,
`import * as fs from "fs/promises"` (read in home, write under `~/.config/herdr-app`).

## herdr-core (`hc`)

`herdr-app --skill` prints the manual for driving this window;
`hc --skill` prints the manual for running and coordinating programs and agents,
including a small herdr-core skill you can install with the user's permission.
`herdr-app commands` lists app commands and arguments without needing an open window.

- `hc ls --json`, `hc read t3 --lines 50`, `hc run --json --cwd <dir> --label k=v -- <cmd>`,
  `hc label t3 k=v`, `hc send t3 "text"`, `hc wait t3`. `hc --help` for all.
- Other machines: `hc machine ls`; `hc run --on <machine> ...` starts a
  terminal there (`vps/t7`), `hc exec --on <machine> git status` runs one
  command there and prints exactly its output.

## Keep in mind

- A task's `cx` belongs to the copy that started it: `cx.notify()` redraws
  that copy, even when another copy woke it (a shared promise).
- One mod per idea. Don't edit `AGENTS.md` or the `.d.ts` files (the app
  rewrites them). Don't touch other mods unless the request
  needs it.

## Yuzora

<!-- yuzora: everything below is Yuzora's; the rest of this file is Herdr's. -->

This window is **Yuzora**, built on the Herdr app: everything above holds,
with these differences.

- This folder is Yuzora's home, `~/.config/yuzora` (`YUZORA_APP_HOME` picks
  another), never Herdr's `~/.config/herdr-app`. The command line is
  `yuzora` (see "The `yuzora` command and updates" below), or the app's own
  program (`Yuzora.app/Contents/MacOS/Yuzora status`, `... do <command>`).
  A `herdr-app` on PATH may be the official Herdr app, which answers for its
  own window, not this one.
- Yuzora keeps its spaces and tabs in herdr-core's `yuzora` layout document,
  so Herdr and Yuzora on one core don't overwrite each other's arrangement.
  Terminals are herdr-core's and both apps see them.
- The default layout is `layouts/yuzora.json`: Herdr's own left side (a 248px
  `sidebar` with the Spaces and agents lists), then the tab bar, the active tab,
  the `panel` column and `yuzora-statusbar`. It is shipped: clone it before
  changing its structure, as with Herdr's layouts.
- Yuzora's four modes (Files, Git, Database, SSH) live in the right dock, which
  the app adds to every layout at load and never writes into a layout file:
  the mode nodes `yuzora-dock-files`, `-git`, `-database`, `-ssh` join the
  `panel-body` stack, and a 40px `yuzora-dock-bar` (the `yuzora-modebar` mod)
  sits right of `panel`. A layout with no `panel` and `panel-body` stack gets a
  drawer at the right edge instead (it opens a mode in a window). The ids
  `yuzora-dock` and `yuzora-dock-*` are reserved (an id like `yuzora-docker` is
  not): don't write them into a layout file (they are ignored there), and select a mode with
  `layout.select {"stack": "panel-body", "child": "yuzora-dock-git"}` plus
  `layout.show {"id": "panel"}`, or with the commands
  `yuzora-modebar.files`, `.git`, `.database`, `.ssh` and `.toggle` (bind them
  in `keys.json`). A layout opts out with `"yuzora": {"dock": "off"}` at its
  root (`"drawer"` forces the drawer). A person's own choice per layout beats
  the layout file's: Settings > Mods > Layouts has Auto / Drawer / Off on every
  layout, saved in Yuzora's `settings.json` as
  `{"shell": {"dock": {"layouts": {"win98": "off"}}}}` (shipped layouts can't
  be edited, so this is how to turn the dock off for one). The default keys
  `cmd-alt-b` (macOS) and `ctrl-alt-b` bind `yuzora-modebar.toggle`; a
  `keys.json` that was edited keeps its own keys, so add the line by hand.
  Settings > General > When Yuzora opens > Right-hand dock
  (`general.startup.dock`: `last`, `none`, `files`, `git`, `database`, `ssh`)
  sets the mode the first dock of a run opens on.
- Themes `yuzora-dark` and `yuzora-light`: the layout shows the one for the
  system's appearance (or settings.json's `appearance`). Design values a theme
  has no name for (diff colors, agent brand colors, Space badge colors, the
  agent states' shapes, sizes) come from `tokens()`, which gives a light theme
  its own text, diff and status colors.
- Mods can import Yuzora's modules: `yuzora:core` (`t`, `tokens`,
  `panes`, `settingsPages`, `getSetting`, `setSetting`, `exec` with its own
  timeout and `on` machine) and `yuzora:shell`, `yuzora:git`, `yuzora:db`,
  `yuzora:ssh`, `yuzora:files`, `yuzora:settings`, `yuzora:perf`. Their
  types are in `yuzora.d.ts` beside `herdr.d.ts` (rewritten on every start).
- Text: a Yuzora mod keeps its strings in `mods/<mod>/i18n/en.json` and
  shows them with `t("key", {name: value})`. Yuzora's interface is English
  only: there is no language setting and no other language file. `locale()`
  always returns `"en"` and `setLocale()` does nothing (both stay so older
  mods keep working); an old `general.locale` in `yuzora/settings.json` is
  ignored.
- A mod offers itself as a tool pane (the new-pane menu) or a settings page
  in its own `mod.json`: `"yuzora": {"pane": {"title": "pane.title"}}`,
  `"yuzora": {"settings": {"title": "settings.title", "section": "git"}}`.
- Yuzora's own settings are `yuzora/settings.json`, one section per area;
  `settings.json` above stays the person's look, sounds and fonts. Passwords
  and keys live in the system's credential store, never in these files.
- Nothing goes to herdr.dev: telemetry is off, feedback opens a GitHub issue
  on NakiriYuuzu/Yuzora, and the app doesn't update itself from Herdr. Only
  herdr-core's own install and update still come from `releases.herdr.dev`.
- Yuzora has no Browser and no language server (LSP).

<!-- yuzora:lane:L1:start -->
### The workspace shell

- The tab bar is `yuzora-shell-tabs`: the current Space's tabs only (the sidebar tells Spaces apart), each with its Space's badge. It has no new-pane or
  split buttons: a pane splits from the buttons in its header or with `pane.split`. The new-pane menu opens with the
  command `yuzora-shell-tabs.new-pane` (a mod's command is written with a dot: `"global": { "alt-n": "yuzora-shell-tabs.new-pane" }`
  in `keys.json`; a colon makes the whole file fail to load; or `herdr-app do yuzora-shell-tabs.new-pane`) and puts
  a tool pane, a terminal or an agent right of or below the focused pane (`pane.split {direction, pane, mod, props}`).
  A mod is listed there when its `mod.json` has `"yuzora": {"pane": {"title": "pane.title"}}`.
  The left side (Herdr's `workspace-chrome`) has no "next layout" row: switch layouts in Settings > Mods > Layouts. Yuzora ignores the old
  default `cmd-shift-l`/`ctrl-shift-l` → `layout.switch` in `global` (also in a keys.json written before it was removed):
  bind another key, or a named `layout.switch:<name>`.
- A person's `space.new` with no arguments but `machine` (a key, a click, a menu, a mod's `run()`) opens the
  `yuzora-new-space` dialog to pick the folder and answers `{"asked": "folder"}`; no Space exists yet. A folder
  that is already a Space's root on that machine selects that Space. Scripts (`herdr-app do`) and calls with
  `name`, `folder`, `tab`, `argv` or `task` create the Space as above. A person's `tab.new` with no arguments
  while no Space is open would start one, so it opens the dialog too.
- Herdr's Files and Changes panels are off in Yuzora until turned on (the welcome's third page, or Settings >
  Yuzora > General > Panels): shared `"workspace.panels"` lists the ones on, and unset means none. The tab bar
  shows a button only for those; Yuzora's right dock has its own Files and Git modes.
- Pane headers are `yuzora-shell-pane`. The yuzora layout's pane frame has one field Herdr's doesn't:
  `"blocked": {"width": 1, "color": "#8a6420"}`, the border while that pane's agent needs a reply.
- `yuzora:shell`: `workspace({...})` (the legacy workspace sidebar's model), `spaceBadges(spaces, shared("yuzora.space.colors"))`
  (each Space's initial and color; when `changed`, write `kept` back to that shared key so the color stays),
  `agentBrand(app, key)`, `gitStatus(cwd, {on})`.
- A mod shows a number on its mode in the mode bar with `setShared("yuzora.badge.<workspace|files|git|database|ssh>", n)`
  (null hides it), and text in the status bar with `setShared("yuzora.status.<lane>", {text, tone})`
  (tone: muted, working, blocked, done or accent).
- The agent states' shapes are `mods/yuzora-sidebar-workspace/status/<working|blocked|done|idle|unknown>.svg`;
  tint them with `tokens().status.<state>.mark`.
- Yuzora's own animations are short (at most 180 ms) and name a speed instead of writing milliseconds:
  `el.transition("left", motion("fast"))` with `motion` from `yuzora:core` (`"fast"` 120 ms, `"base"` 160 ms, both
  ease-out; `tokens().motion` lists them). They go through `transition`, which follows the system's reduce-motion
  setting; `loop` does not, and a mod cannot read that setting, so a Yuzora mod does not use `loop`.
<!-- yuzora:lane:L1:end -->

<!-- yuzora:lane:L2:start -->
### Git (`yuzora:git`)

- The Git mode follows the shown space and the focused pane's folder in the
  frame they change. It keeps what it showed of each repository it has seen
  (up to 8), so a repository seen before is shown at once and read again
  behind it; until that reading is back, a click that would stage, discard,
  commit or push there does nothing. The commit message, author and Amend are kept per
  repository, and a repository with a message being written, an action going
  on or an action's error is not dropped from the 8.
- The right dock's Git mode is `yuzora-sidebar-git`. Tool panes: `yuzora-git-log`
  (props `{cwd, on, filter?, compare?}`; `filter: {paths: [p], follow: true}`
  is a file's history), `yuzora-git-diff` (`{cwd, on, path, mode: "unstaged" |
  "staged" | "commit" | "compare", rev?, to?}`), `yuzora-git-merge`,
  `yuzora-git-rebase` (`{cwd, on, base}`), `yuzora-git-blame` (`{cwd, on,
  path, rev?}`), `yuzora-git-repo` (`{cwd, on}`). Open one with
  `herdr-app do tab.new --name Log --mod yuzora-git-log --props '{"cwd": "/path/to/repo"}'`.
- In a mod, `open(cwd, on)` from `yuzora:git` gives the repository (`id`,
  `root`, `machine`); every other function takes that `id`. `changed(id, since)`
  resolves when the repository changed. Git runs in the app (never on the UI
  thread), with locks for writes and a timeout; don't run `git` with `exec`
  for things `yuzora:git` does.
- Git settings are the `git` section of `yuzora/settings.json` (`settings()` /
  `setSettings({...})` in `yuzora:git`): e.g. `logRecentCount`,
  `forceWithLease`, `protectedBranches` (a regex), `updateMethod`,
  `swapSidesInRebase`.
- A file clicked in the sidebar, a commit's files, Annotate or the repository
  pane shows its diff as the `git` settings say: `diffOpenIn` is `"modal"`
  (the default: a dialog over the window, whose Keep button moves the diff into
  a pane) or `"pane"`; `diffKeepIn` is where Keep and `"pane"` put it,
  `"split"` (the default: beside the focused pane) or `"tab"`; with
  `diffFollowPane` (default `true`) a diff pane already open shows the next
  file instead. A diff pane's props can also set its view: `layout: "side" |
  "unified"`, `whitespace: "none" | "trim" | "all"`, `context` (lines; `-1` is
  the whole file).
- Side by side, a diff's two sides share its width by one ratio, the left
  side's share, half and half by default. Drag the divider between them to
  change it (it stops at 15% and 85%); double-click the divider to put half
  back. The ratio is kept across restarts, for every diff, in shared state
  `yuzora.git.diff.split`: a number, held between `0.15` and `0.85`; no value,
  `null` or anything that is not a number is `0.5`.
  `setShared("yuzora.git.diff.split", 0.7)` sets it and
  `setShared("yuzora.git.diff.split", null)` resets it. A line wider than its
  side is cut there; a sideways swipe, or the bar under the lines when they
  need one, scrolls both sides together.
- In a diff, click a row's text to select the row and Shift-click another to
  extend the selection to it, or press in a row's text and drag: the selection
  runs from the row pressed to the row under the pointer, up or down, and is
  still there when the button is let go. A press is a click until the pointer
  has gone 4 px from where it landed or is over another row: a button held
  still, at the edge of the list too, scrolls nothing and selects nothing but
  the row clicked. From then on, with the pointer in the top or bottom 20 px
  of the list (or past it) the list scrolls and the selection goes on with it:
  a row every 50 ms (20 rows a second) at the bottom edge, and from the header
  above the list up to three rows a tick (60 rows a second), faster only under
  a taller strip (eight a tick at most). The drag ends, and keeps the
  selection, when the button is let go, when a pointer move says the button is
  up (an app-wide shortcut pressed during the drag keeps the release from
  reaching the pane), when the pointer leaves the pane, when the pane loses
  the keyboard, or at a key that reaches the pane (Cmd+C copies what is
  selected so far). A pointer left still at the edge after a release the pane
  never saw goes on scrolling the list until it moves or the pane loses the
  keyboard. The press that begins a selection decides the side: side by side,
  only that side's lines are copied; unified copies the lines as shown (a
  deleted line from the left, an added one from the right). A line number
  picks the line for staging where the diff can stage (a drag from it picks
  and selects nothing), and selects the row where it cannot; a fold opens the
  whole file when clicked and starts no selection; the divider and the thumb
  of the bar under the lines drag themselves, and a press on that bar's track
  starts nothing (the bar under unified's lines is the window's own, and a
  press on it is a press on the row behind it). Cmd+C (Ctrl+C on Windows and Linux) copies
  the selected rows' lines as the file has them (tabs, trailing spaces and
  wide characters kept; no line numbers, no `+` or `-`) and says so in a
  toast; with nothing selected the key is left alone. Right-click a row (it
  joins the selection, or replaces it when it is not in it) for three items:
  Copy (the selected lines), Copy hunk (the hunk under the pointer as it is
  after the change; none for a hunk that only deletes) and Copy as patch (a
  unified patch `git apply` takes, of every hunk the selection touches). The
  selection is by row, not by character; it goes when the diff is read again
  (a drag in progress ends with it) or its layout, whitespace or context
  changes. Scrolling, dragging the divider and the bar under the lines leave
  it as it is.
- A diff, merge or rebase pane follows what its own space asks for, never another
  space's, and an opener (the sidebar, the log, Annotate, the repository pane) looks
  for the pane among the tabs of the space shown, so a space's pane is never
  focused or changed from another space. A request is the pane's props plus `at`
  (a time in ms), kept per space in shared `yuzora.git.diff.<space id>` (also
  `yuzora.git.merge.<space id>`, `yuzora.git.rebase.<space id>`). A request written
  to the plain key, `setShared("yuzora.git.diff", {cwd, path, mode, at})`, names no
  space: the pane of every space follows it, the newer of it and its space's own.
  A plain request is an order for this run, not state kept across runs: shared is
  kept in a file, and the first time a pane reads in a run (any of the three kinds)
  it drops the plain requests an earlier run left, so none is shown in a space that
  never asked. What you write after that is followed, with or without `at`; what you
  wrote before any of these panes drew in this run is dropped too. A request
  to keep across restarts goes in the `<space id>` key.
- A space can hold several repositories. The picker at the top of the Git
  sidebar follows the focused pane's repository (the default) or keeps one
  pinned for the space: `setShared("yuzora.git.repo.<space id>", {root, on})`
  pins it (kept across restarts), `null` follows the focus again; the command
  `yuzora-sidebar-git.pick-repo` opens the picker for the keyboard (↑↓, Enter,
  Esc). The badge, `yuzora.status.git`, and the Log, diff, branches and push
  opened from the sidebar follow the repository picked. A pinned repository
  that is gone (its folder, or the `.git` in it, as the file system tells) is
  unpinned with a note; one that can't be looked at now stays pinned, with the
  error. `repos(folder, on, cwds)` lists a space's repositories without
  running git (the file system only; on a machine, one call): the ones
  `folder` and each of `cwds` are in, then folders one and two below `folder`
  with a `.git`, at most 50 (names with a line feed in them are passed over).
  A `folder` gone for sure has none; one that can't be looked into now (no
  permission, say) rejects.
<!-- yuzora:lane:L2:end -->

<!-- yuzora:lane:L3:start -->
### Databases (`yuzora:db`)

- Databases (PostgreSQL, SQL Server, SQLite) live in the right
  dock's `yuzora-sidebar-database` mode. Saved connections are the `db` section of
  `yuzora/settings.json` (`connections`, and `maxRows`, the most rows a result
  keeps: 100000 by default). Passwords are in the system's credential store,
  never in that file; don't write one there. Query history is
  `yuzora/db-history.json` (statements only).
- Open a table or a query for the person with `pane.split` or `tab.new`:
  `{"mod": "yuzora-db-table", "props": {"connection": "<id>", "database": null,
  "schema": "public", "name": "orders", "tab": "data"}}` (`tab`: `data`,
  `structure` or `sql`), or `{"mod": "yuzora-db-query", "props":
  {"connection": "<id>", "database": null, "sql": "select …"}}`. Connection
  ids are in the settings file.
- Commands go to the shown pane: `yuzora-db-query.run` (the statement under
  the cursor), `.run_all`, `.stop`; `yuzora-db-table.data`, `.structure`,
  `.sql`, `.run`, `.stop`; `yuzora-sidebar-database.next`, `.prev`,
  `.refresh`, `.add`, `.query`. Edited cells are written back only when the
  person presses Write back or Cmd+S (Ctrl+S) in the table; nothing is saved
  by itself. A write-back that waits (on a lock, on the connection) shows
  Stop: stopping it writes nothing, unless the message says the server had
  already been asked to commit, and the edits stay in the cells. Running the
  table again with edits not yet written asks first.
- A table is read and written back on a connection of its own, apart from the
  one its SQL tab runs on. What a SQL tab did (`SET`, `USE`, a temporary
  table) stays on the SQL tab's connection and does not change what the table
  reads or saves. A transaction the SQL tab left open is not seen by the
  table, and it can hold the table up (next bullet). The table's Stop and the
  SQL tab's Stop each end only their own statement, and a write-back that
  waits on another connection's lock does not hold the SQL tab up. A pane
  still has one job at a time, so running a statement in the SQL tab replaces
  what the pane showed of the table, as it always did. A pane that uses both
  holds one connection more than a pane that only opens a table (closing the
  pane closes all of them).
- A transaction left open in the SQL tab (`BEGIN` without `COMMIT` or
  `ROLLBACK`) is the one thing that reaches across the two connections, and
  only as a lock: while it holds locks the table needs, the table's read or
  write-back in the same pane waits for it. SQL Server waits without end
  (the table stays on Loading or Writing until it is stopped); the locks of an
  `UPDATE` block the table's read of those rows. SQLite fails the table's
  write-back with `database is locked` after about 5 seconds, and a read-only
  `BEGIN; SELECT …` left open is enough to cause it. PostgreSQL does not
  block a read except behind a strong lock (an uncommitted `ALTER TABLE`), but
  a write-back to a row the transaction changed waits. To recover from a read
  that waits, go to the SQL tab, type `COMMIT` or `ROLLBACK`, and press Run:
  while the table is still being read, Run stops that read first and then runs
  the statement (it asks first when the table has unsaved edits). A write-back
  that waits is not stopped by Run (it raises a bar with Keep waiting and
  Stop saving): press Stop saving. A statement of
  the person's own that is still running has to be stopped with Stop. Then run
  the table again. What the SQL tab has not committed is not in the table either. Tell
  the person this when a table will not load or save after they used the SQL
  tab.
- Connections count against a login's limit (`too many connections …`; the
  server's message is shown). With a very small limit either side can fail,
  not only the SQL tab: the SQL tab fails when a table is open, and the
  table fails to open when the SQL tab holds the room, for example after the
  table's connection was lost (a Stop of a write-back, an idle timeout, the
  network) and the SQL tab connected in the meantime. Running the table again
  in that pane keeps failing; close the pane (that releases all of its
  connections) and open the table in a new one. In the other direction (the
  SQL tab fails because a table is open) the table's connection is still
  there: run the table again and it opens and saves.
- Clearing a cell (Backspace, then Enter) makes it NULL where the column's
  type has no empty value (numbers, dates, times, `uuid`, `bool`, `json`),
  and the cell shows NULL before anything is written; a text column keeps the
  empty string, and a NOT NULL column is refused by the server. SQLite keeps
  the empty string in every column.
- A date typed into a SQL Server `datetime` or `smalldatetime` cell is saved
  as the date it says or refused before anything is sent. A year-first date
  (`2024-01-06`, `2024/1/6`, `2024.01.06`, `20240106`, with a time before or
  after it, `T` or `Z` included) is read the same under every language and
  `DATEFORMAT`; a year-first date with anything else the app cannot read for
  sure (`20240106 9:5`) is refused. A zone offset such as `+08:00` goes to the
  server, which refuses it for these two types. Any other style (`1/6/2024`)
  is read by the login's own language, as the server does.
<!-- yuzora:lane:L3:end -->

<!-- yuzora:lane:L4:start -->
### SSH / SFTP (`yuzora:ssh`)

- Hosts come from `~/.ssh/config` (read only), Yuzora's own hosts (`yuzora/settings.json` → `ssh.profiles`; passwords in the system keychain) and herdr machines. `hosts(machines)` lists them with `state`, `latencyMs` and the verified host key; pass `hc machine ls --json`'s `machines`.
- `connect(host)` may need the person: a new or changed host key, a password, a passphrase, keyboard-interactive answers. Those wait in `prompts()`; a mod that shows them calls `claimPrompt(id)` and opens `yuzora-ssh-prompt` with `props: { id }` (the SSH sidebar and the SFTP pane already do).
- Yuzora writes host keys only to `yuzora/known_hosts` here; `~/.ssh/known_hosts` is read, never written.
- `list(side, path)` (`side` null: this computer), `transfer({host, direction, from, toDir, preserve})` and `transfers()`, `openRemote(host, path)` (a local copy whose saves upload), `openTunnel(host, remoteHost, remotePort)`.
- Open SFTP for a host beside the focused pane: `run("pane.split", { direction: "right", mod: "yuzora-sftp", props: { host: "ssh:lab-01" } })`.
- The status bar line is `shared("yuzora.status.ssh")`; the mode badge `shared("yuzora.badge.ssh")` counts transfers in progress.
<!-- yuzora:lane:L4:end -->

<!-- yuzora:lane:L5:start -->
### Files and the editor

- The right dock's Files mode shows the focused pane's repo. Files open beside the
  focused pane: `yuzora-editor` (code), `yuzora-markdown` (`.md`: preview or
  source), `yuzora-files-image` (images and SVG). Open one yourself with
  `pane.split {direction: "right", mod: "yuzora-editor", props: {path: "/abs/file.rs", line: 12}}`
  (`on`: a machine for a remote space's file).
- The editor follows the disk: a file with no unsaved changes reloads when it
  changes; with unsaved changes the person is asked. While you (an agent) are
  working in a folder that holds an open file you just changed, its pane shows
  "<agent> is editing too".
- `yuzora:files` (types in `yuzora.d.ts`): `listDir`, `gitStatus`, `findFiles`,
  `grep`, `openDocument` / `documentState` / `saveDocument`, the native
  components `Editor`, `Markdown`, `ImageView`, and more.
- Files on another machine can be browsed, opened and saved (the file is replaced
  whole, a change made there since it was read is a conflict, and a save never
  gives the file wider access than it had). A file that a save could not give
  the same owner, group and ACL opens read-only. Creating, renaming and
  deleting there are not available yet.
<!-- yuzora:lane:L5:end -->

<!-- yuzora:lane:L6:start -->
### Settings, mods and templates (`yuzora:settings`)

- Settings (`app.settings`, ⌘,) open on Yuzora's General page;
  `{"section": "templates"}` opens another (a Yuzora page's section or mod
  name, or one of Herdr's sections). A mod becomes a page with
  `"yuzora": {"settings": {"title": "settings.title", "icon": "icon.svg", "order": 500, "section": "git"}}`
  and is shown beside the settings nav with `props.section`. General is order
  0, Templates 800, Mods 900.
- Agent templates are herdr-core's (`~/.config/herdr-core/templates/<name>.md`,
  `hc templates`). The Templates settings page edits them on any machine.
  `yuzora-settings-launch` starts one:
  `openOverlay({mod: "yuzora-settings-launch", kind: "dialog", width: 560, props: {template, machine, space, tab, task}})`,
  every prop optional. Without the dialog, `yuzora:settings`
  `runTemplate({template, task, machine, space, tab, cwd, name})` runs
  `hc run` with `herdr.space`/`herdr.tab` labels, so the terminal lands in
  that space and tab (a tab id the app doesn't know opens a new tab).
- `yuzora:settings` also lists mods and layouts with where they came from
  (`listMods()`), checks one (`checkMod(name)`) and installs a folder
  (`planInstall(path)` reads it and shows what it would install;
  `installFolder(plan.token)` installs exactly what that plan read). Mods
  are trusted code: show the person the plan and ask before installing.
<!-- yuzora:lane:L6:end -->

<!-- yuzora:lane:L7:start -->
### The `yuzora` command and updates

- In Yuzora the command line program is `yuzora`: the same program as
  `herdr-app` in this guide, so run `yuzora status`, `yuzora ls`,
  `yuzora do …`, `yuzora shot …` and `yuzora check …`. A `herdr-app` on
  PATH, if there is one, is the official Herdr app: it drives another window
  with another home, never this one.
- Yuzora updates itself from the GitHub releases of NakiriYuuzu/Yuzora, and
  only from a manifest signed with the release key. A beta build only
  updates to a stable release, never to another beta. A stable build
  follows stable; `"update": {"channel": "beta"}` in
  `yuzora/settings.json` makes it take pre-releases too. An update waiting
  shows in Settings and goes in when Yuzora restarts.
- What's new is English, from the bundled `CHANGELOG.md`. A newer release's
  notes are kept in `update-notes-cache.md`. (`CHANGELOG.zh-TW.md` ships in
  the repo and the release for people who read Chinese; the app does not
  show it.)
<!-- yuzora:lane:L7:end -->

<!-- yuzora:lane:L8:start -->
### Resource usage

- The chip in the bottom-right corner is `yuzora-perf`: what this computer's Yuzora uses (the app, its herdr-core
  with the terminals and agents in it, and the tools the app starts). `yuzora-perf.toggle` (a key in keys.json, or
  `herdr-app do`) opens or closes the details. Remote (SSH) Spaces are not measured.
- `yuzora:perf`: `tick(focused)` samples on a worker thread (at most once every 1.5 s, whatever the number of
  callers) and `state()` reads the last sample without sampling: `{snapshot: {total, app, core, tools, core_pid,
  core_error}, health: {attempts, failures, empty, skipped, last_error}, cost}`. Each category is
  `{cpu, memory, count}`: cpu is one core = 100, summed; memory is RSS in bytes. The categories add up to total.
<!-- yuzora:lane:L8:end -->

<!-- yuzora:lane:L9:start -->
### Beside the Herdr app

The person may run the official Herdr app on the same herdr-core. Its spaces and tabs are its own (layout document
`herdr-app`). When both apps use the same core session (the default one, unless `HC_SESSION` picks another), terminals
and machines are shared; templates are shared whatever the session.

- Terminals Herdr opened show up here in spaces of their own, and Yuzora's show up in Herdr. `space.close`,
  `tab.close`, `pane.close` and `terminal.kill` end the terminal in herdr-core, so it is gone from both apps. Ask the
  person before closing a space, tab or pane, or running `terminal.kill`, on what may be Herdr's terminals.
- Don't use `hc server stop` to stop the core: while either app is open it starts again within a second, and every
  program in its terminals ends. To stop it for good, the person quits both apps first.
- Herdr's mods and settings aren't imported. A Herdr mod works here once its folder is copied from `mods/<name>` in
  Herdr's home to `mods/<name>` in this folder (never `installed.json`). If `mods/<name>` is already here, don't copy
  over it: ask the person, or point them to Settings › Mods › Install from folder…, which picks a free name. Herdr's
  home is `$XDG_CONFIG_HOME/herdr-app` when that is set, else `~/.config/herdr-app`, unless the person moved it with
  `HERDR_APP_HOME` (ask if it isn't there). A copied mod's `fs` writes under Herdr's home are refused here, but the commands it runs aren't limited.
  `yuzora-*` mods don't load in Herdr.
- `hc machine add`, `rm` and `rename`, and edits in herdr-core's templates folder, change them for Herdr too. The
  folder is `$XDG_CONFIG_HOME/herdr-core/templates/` when that is set, else `%APPDATA%\herdr-core\templates` on
  Windows and `~/.config/herdr-core/templates/` elsewhere (`hc templates` lists what's there).
<!-- yuzora:lane:L9:end -->
