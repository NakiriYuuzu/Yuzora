// File/folder type icons.
// Source: material-icon-theme (VSCode Material Icon Theme), MIT License.
// https://github.com/material-extensions/vscode-material-icon-theme

import bunIcon from "material-icon-theme/icons/bun.svg"
import cIcon from "material-icon-theme/icons/c.svg"
import consoleIcon from "material-icon-theme/icons/console.svg"
import cppIcon from "material-icon-theme/icons/cpp.svg"
import cssIcon from "material-icon-theme/icons/css.svg"
import databaseIcon from "material-icon-theme/icons/database.svg"
import dockerIcon from "material-icon-theme/icons/docker.svg"
import documentIcon from "material-icon-theme/icons/document.svg"
import fileIcon from "material-icon-theme/icons/file.svg"
import folderOpenIcon from "material-icon-theme/icons/folder-open.svg"
import folderIcon from "material-icon-theme/icons/folder.svg"
import gitIcon from "material-icon-theme/icons/git.svg"
import goIcon from "material-icon-theme/icons/go.svg"
import hIcon from "material-icon-theme/icons/h.svg"
import hppIcon from "material-icon-theme/icons/hpp.svg"
import htmlIcon from "material-icon-theme/icons/html.svg"
import imageIcon from "material-icon-theme/icons/image.svg"
import javaIcon from "material-icon-theme/icons/java.svg"
import javascriptIcon from "material-icon-theme/icons/javascript.svg"
import jsonIcon from "material-icon-theme/icons/json.svg"
import lockIcon from "material-icon-theme/icons/lock.svg"
import markdownIcon from "material-icon-theme/icons/markdown.svg"
import nodejsIcon from "material-icon-theme/icons/nodejs.svg"
import pdfIcon from "material-icon-theme/icons/pdf.svg"
import pythonIcon from "material-icon-theme/icons/python.svg"
import reactIcon from "material-icon-theme/icons/react.svg"
import reactTsIcon from "material-icon-theme/icons/react_ts.svg"
import rustIcon from "material-icon-theme/icons/rust.svg"
import sassIcon from "material-icon-theme/icons/sass.svg"
import svelteIcon from "material-icon-theme/icons/svelte.svg"
import svgIcon from "material-icon-theme/icons/svg.svg"
import tomlIcon from "material-icon-theme/icons/toml.svg"
import tsconfigIcon from "material-icon-theme/icons/tsconfig.svg"
import tuneIcon from "material-icon-theme/icons/tune.svg"
import typescriptIcon from "material-icon-theme/icons/typescript.svg"
import vueIcon from "material-icon-theme/icons/vue.svg"
import xmlIcon from "material-icon-theme/icons/xml.svg"
import yamlIcon from "material-icon-theme/icons/yaml.svg"
import zipIcon from "material-icon-theme/icons/zip.svg"

// Exact filename overrides, matched before extension lookup (case-insensitive).
const EXACT_NAME_ICONS: Record<string, string> = {
    "package.json": nodejsIcon,
    "tsconfig.json": tsconfigIcon,
    "cargo.toml": rustIcon,
    dockerfile: dockerIcon,
    ".gitignore": gitIcon,
    "bun.lock": bunIcon
}

// Extension → icon lookup, keyed without the leading dot (lowercase).
const EXTENSION_ICONS: Record<string, string> = {
    ts: typescriptIcon,
    tsx: reactTsIcon,
    js: javascriptIcon,
    jsx: reactIcon,
    mjs: javascriptIcon,
    cjs: javascriptIcon,
    json: jsonIcon,
    jsonc: jsonIcon,
    rs: rustIcon,
    py: pythonIcon,
    md: markdownIcon,
    css: cssIcon,
    scss: sassIcon,
    html: htmlIcon,
    yml: yamlIcon,
    yaml: yamlIcon,
    toml: tomlIcon,
    lock: lockIcon,
    png: imageIcon,
    jpg: imageIcon,
    jpeg: imageIcon,
    gif: imageIcon,
    webp: imageIcon,
    svg: svgIcon,
    sh: consoleIcon,
    bash: consoleIcon,
    zsh: consoleIcon,
    go: goIcon,
    java: javaIcon,
    c: cIcon,
    cpp: cppIcon,
    h: hIcon,
    hpp: hppIcon,
    vue: vueIcon,
    svelte: svelteIcon,
    sql: databaseIcon,
    xml: xmlIcon,
    txt: documentIcon,
    pdf: pdfIcon,
    zip: zipIcon,
    env: tuneIcon
}

function iconForFile(fileName: string): string {
    const lower = fileName.toLowerCase()
    if (Object.hasOwn(EXACT_NAME_ICONS, lower)) return EXACT_NAME_ICONS[lower]
    if (lower === ".env" || lower.startsWith(".env.")) return tuneIcon
    const dotIndex = fileName.lastIndexOf(".")
    const ext = dotIndex > -1 ? lower.slice(dotIndex + 1) : ""
    return Object.hasOwn(EXTENSION_ICONS, ext) ? EXTENSION_ICONS[ext] : fileIcon
}

export function FileIcon({
    fileName,
    isDirectory,
    isOpen,
    className
}: {
    fileName: string
    isDirectory?: boolean
    isOpen?: boolean
    className?: string
}) {
    const src = isDirectory ? (isOpen ? folderOpenIcon : folderIcon) : iconForFile(fileName)
    return <img src={src} alt="" aria-hidden="true" draggable={false} className={className} />
}
