import { parseRuntimeScope } from "@/lib/herdrProvider";
import { canonicalPathKey } from "@/lib/paths";
/** Display preferences share a project only within its owning host. */
export function spacePresentationKey(scope: string, path: string): string {
  return `space:${JSON.stringify([parseRuntimeScope(scope).hostId, canonicalPathKey(path)])}`;
}
