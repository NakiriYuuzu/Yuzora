// Path helpers, with no imports (a test runs them as they are).
// A Windows path (`C:\…`, `\\server\…`) may use `\` as well as `/`;
// anywhere else `\` is part of a name.
const windows = path => /^([A-Za-z]:[\\/]|\\\\)/.test(path);
const lastSlash = path => (windows(path) ? Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) : path.lastIndexOf("/"));

export const join = (dir, name) => (dir.endsWith("/") ? dir + name : `${dir}/${name}`);
export const parent = path => {
  const i = lastSlash(path);
  if (i < 0) return "";
  // A root keeps its slash: "/", or a drive's "C:\".
  const dir = path.slice(0, i);
  return dir === "" || /^[A-Za-z]:$/.test(dir) ? path.slice(0, i + 1) : dir;
};
export const base = path => path.slice(lastSlash(path) + 1);
/** `path` relative to `root` ("" for root itself), or null outside it. */
export const rel = (root, path) => {
  if (path === root) return "";
  const prefix = root.endsWith("/") ? root : root + "/";
  return path.startsWith(prefix) ? path.slice(prefix.length) : null;
};
