// No imports (a test runs this as it is).
// `gitStatus` answers null for a folder that is not in a repo and rejects when
// it could not tell (git failed or timed out): the tree says so for the second.
export async function readGit(gitStatus, cwd, options) {
  try {
    return { git: (await gitStatus(cwd, options)) ?? null, failed: null };
  } catch (e) {
    return { git: null, failed: String(e?.message ?? e) };
  }
}
