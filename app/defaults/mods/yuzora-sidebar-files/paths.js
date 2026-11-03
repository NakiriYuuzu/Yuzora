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
/**
 * The sidebar's heading for `root`: `dir`, the folder it is in ending in its
 * separator ("~/" and so on for the home folder), then `name`, the folder's
 * own; `branch` is null for none. `home` is this machine's home: give null
 * for another machine's path. The home folder itself is "~" and a root ("/",
 * "C:\") its own name, with no `dir`.
 */
export const heading = (root, home, branch) => {
  const under = p => home && (p === home || p.startsWith(home + "/") || (windows(home) && p.startsWith(home + "\\")));
  const tilde = p => (under(p) ? "~" + p.slice(home.length) : p);
  const dir = parent(root);
  const name = home && root === home ? "~" : !dir || dir === root ? root : base(root);
  if (name === root || name === "~") return { name, dir: "", branch: branch || null };
  const above = tilde(dir);
  const sep = /[\\/]$/.test(above) ? "" : windows(root) && !root.includes("/") ? "\\" : "/";
  return { name, dir: above + sep, branch: branch || null };
};
