import { renderToStaticMarkup } from "react-dom/server";
import { readFile, writeFile } from "node:fs/promises";
import { SpaceCharacter } from "../src/app/workbench/SpaceCharacter";
const markup = [
  { shell: "box", face: "curious", detail: "patch" },
  { shell: "cloud", face: "smile", detail: "freckles" },
  { shell: "round", face: "sleepy", detail: "none" },
]
  .map((character) =>
    renderToStaticMarkup(
      <div className="site-companion">
        <SpaceCharacter
          character={
            character as Parameters<typeof SpaceCharacter>[0]["character"]
          }
        />
      </div>,
    ),
  )
  .join("");
const path = "site/index.html";
const html = await readFile(path, "utf8");
const start = "<!-- companions:start -->",
  end = "<!-- companions:end -->";
const updated = html.includes(start)
  ? html.replace(
      /<!-- companions:start -->[\s\S]*?<!-- companions:end -->/,
      `${start}${markup}${end}`,
    )
  : html.replace(
      '<div id="site-companions" aria-hidden="true"></div>',
      `<div id="site-companions" aria-hidden="true">${start}${markup}${end}</div>`,
    );
await writeFile(path, updated);
const source = await readFile(
  "src/app/workbench/space-agent-sidebar.css",
  "utf8",
);
const shape = source.slice(
  source.indexOf(".space-character-art { display:"),
  source.indexOf(".space-character-dialog {"),
);
const motion = source.slice(
  source.indexOf(".space-character-art { --character-lift:"),
  source.indexOf(".space-tree-attention {"),
);
await writeFile(
  "site/assets/brand/companions.css",
  `/* Generated from the app's SpaceCharacter styles. */\n${shape}\n${motion}`.trimEnd() + "\n",
);
