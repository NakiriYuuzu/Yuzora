import { parseRuntimeScope } from "@/lib/herdrProvider";
import { canonicalPathKey } from "@/lib/paths";
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity";
import { isWindowsPlatform } from "@/lib/platform";
import type { HerdrNamedSession } from "@/lib/herdrTypes";
import i18n from "@/lib/i18n";

export function runtimeSessionLabel(scope: string, session?: HerdrNamedSession): string {
  const identity = parseRuntimeScope(scope);
  const host = identity.hostId === LOCAL_HOST_ID
    ? i18n.t(isWindowsPlatform() ? "windowsHost" : "localHost", { ns: "spaceTree" })
    : session?.hostLabel;
  return [host, session?.name ?? identity.sessionName].filter(Boolean).join(" · ");
}

/** Display preferences share a project only within its owning host. */
export function spacePresentationKey(scope: string, path: string): string {
  return `space:${JSON.stringify([parseRuntimeScope(scope).hostId, canonicalPathKey(path)])}`;
}
