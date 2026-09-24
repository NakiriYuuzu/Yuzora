# Kitty renderer local browser E2E — 2026-09-23

Controlled-IPC fixture with production HerdrNativeDialog/xterm/Kitty renderer at http://127.0.0.1:1431/fixtures/herdr-parity.html. These checks do not prove real HERDR runtime behavior.

Browser operations ended immediately when the user restricted further testing to Chrome/computer-use. The following checks predate that instruction; no agent-browser operations were performed afterward.

## Confirmed bug and fix

`src/terminal/kittyRenderer.ts`: `.xterm-rows` had `z-index: 1` but computed `position: static`, so negative-z graphics covered text. Set `position: relative` while renderer is mounted and restore both original position and z-index on disposal.

## Observed checks

- Before fix: `kitty-negative-before.png` shows z=-1 checkerboard obscuring `Text under image` despite text remaining present in xterm DOM.
- After fix: `kitty-negative-after.png` shows text on top of z=-1 checkerboard.
- Explicit ANSI yellow background and black text appear over negative-z graphics: `kitty-negative-background-after.png`.
- z=0 graphics cover both text and explicit ANSI background: `kitty-zero-background-after.png`.
- Viewport resize from 1280x577 to 900x700 resized both canvas layers to 842x560. Checkerboard remains aligned and correctly sized: `kitty-resize-after.png`. Terminal resize IPC recorded, final observed cols=117, rows=35.
- Delete placement command `ESC_Ga=d,d=i,i=42,q=2;ESC\\` leaves both canvas layers with exactly zero nontransparent pixels: `kitty-delete-after.png`.
- Initial mount geometry remains a separate dialog issue: xterm screen width 792 vs wrapper width 1228; after viewport resize screen842 vs wrapper848. Root notified to fix fit timing in HerdrNativeDialog.

Canvas pixel checks were awaited after requestAnimationFrame; screenshots were captured only after visible output settled.

## Remaining visual checks

Scroll/reset, dark theme, close/reopen, and complete fit correction must be continued through the user-selected Chrome/computer-use tools by root. Native dialog has scrollback:0, so scrolling the underlying HERDR history is a distinct real-runtime scenario.

## Non-browser verification

- `bun run test src/terminal/kittyProtocol.test.ts`: 9/9 passed.
- `bunx eslint src/terminal/kittyRenderer.ts src/terminal/kittyProtocol.ts`: passed.
