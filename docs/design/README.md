# DESIGN HANDOFF: READ THIS FIRST

This directory contains HTML/CSS/JS prototypes exported from a design tool. They are visual references, not production architecture or current product terminology.

## Primary prototype

Read [`project/Yuuzu Workbench.dc.html`](project/Yuuzu%20Workbench.dc.html) in full and follow its local imports before implementing visual changes. Match its dimensions, tokens and layout where they still agree with the production app.

The production source and current ADE × HERDR behavior are authoritative whenever this prototype differs. In particular, the Workspace rail projects HERDR Spaces, the ADE sidebar contains named Sessions plus Attention and Agents, and HERDR terminal pages own recursive BSP panes.

## Working rules

- Recreate the visual result in the target stack; do not copy prototype internals blindly.
- Check `src/styles.css` and current React components before adopting tokens or UI structure.
- Do not infer current runtime behavior from this handoff.
- Render or capture the prototype only when visual comparison is explicitly needed.
