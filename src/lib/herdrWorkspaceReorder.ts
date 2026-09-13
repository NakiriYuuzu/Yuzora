/**
 * Convert a drop position in the current workspace list to HERDR's
 * `workspace.move.insert_index` semantics.
 *
 * HERDR removes the source first, then inserts at the final index in the
 * shortened list. The returned index is therefore the post-removal index.
 */
export function herdrWorkspaceInsertIndex(
  sourceIndex: number,
  targetIndex: number,
  afterTarget: boolean,
  length: number
): number | null {
  if (
    !Number.isInteger(sourceIndex) ||
    !Number.isInteger(targetIndex) ||
    !Number.isInteger(length) ||
    sourceIndex < 0 ||
    targetIndex < 0 ||
    sourceIndex >= length ||
    targetIndex >= length ||
    length < 1
  ) return null

  const requestedIndex = targetIndex + (afterTarget ? 1 : 0)
  const finalIndex = sourceIndex < requestedIndex ? requestedIndex - 1 : requestedIndex
  return sourceIndex === finalIndex ? null : finalIndex
}
