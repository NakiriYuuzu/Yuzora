// File-type icons (icons/files/<icon>.svg), with no imports (a test runs this
// as it is). The tables are built once, when the mod loads; a row only looks
// its name up. `color` names a theme color (theme()[color]): light and dark
// themes name the same ones, so the icons follow both.
const BY_NAME = new Map(), BY_EXT = new Map();
const add = (map, keys, icon, color) => { for (const key of keys.split(" ")) map.set(key, { icon, color }); };

add(BY_EXT, "rs", "code", "ansi3");
add(BY_EXT, "ts tsx mts cts", "code", "ansi4");
add(BY_EXT, "js jsx mjs cjs", "code", "ansi11");
add(BY_EXT, "py pyi", "code", "ansi4");
add(BY_EXT, "go", "code", "ansi6");
add(BY_EXT, "html htm xml", "code", "ansi1");
add(BY_EXT, "css scss sass less", "code", "ansi5");
add(BY_EXT, "c h cc cpp hpp java kt swift rb php cs lua zig sql vue svelte", "code", "muted");
add(BY_EXT, "json jsonc json5", "json", "ansi11");
add(BY_EXT, "md mdx markdown", "markdown", "muted");
add(BY_EXT, "txt log rst", "text", "muted");
add(BY_EXT, "toml yaml yml ini conf cfg properties", "config", "ansi6");
add(BY_EXT, "png jpg jpeg gif webp bmp ico tif tiff avif svg", "image", "ansi13");
add(BY_EXT, "sh bash zsh fish ps1", "terminal", "ansi2");
add(BY_EXT, "lock lockb", "lock", "dim");

add(BY_NAME, "cargo.toml", "package", "ansi3");
add(BY_NAME, "package.json", "package", "ansi2");
add(BY_NAME, "package-lock.json pnpm-lock.yaml", "lock", "dim");
add(BY_NAME, "dockerfile containerfile .dockerignore docker-compose.yml docker-compose.yaml compose.yml compose.yaml", "container", "ansi4");
add(BY_NAME, ".gitignore .gitattributes .gitmodules .gitkeep", "git", "ansi1");
add(BY_NAME, ".env .editorconfig", "config", "ansi6");
add(BY_NAME, "makefile justfile", "terminal", "ansi2");

const FOLDER = { icon: "folder", color: "dim" }, FOLDER_OPEN = { icon: "folder-open", color: "dim" };
const FILE = { icon: "file", color: "dim" };

/** The icon and theme color for a file or folder `name` (not a path). */
export function fileIcon(name, dir = false, open = false) {
  if (dir) return open ? FOLDER_OPEN : FOLDER;
  const lower = name.toLowerCase();
  const named = BY_NAME.get(lower);
  if (named) return named;
  // Dockerfile.dev, .env.local
  if (lower.startsWith("dockerfile.")) return BY_NAME.get("dockerfile");
  if (lower.startsWith(".env.")) return BY_NAME.get(".env");
  const dot = lower.lastIndexOf(".");
  return (dot > 0 && BY_EXT.get(lower.slice(dot + 1))) || FILE;
}
