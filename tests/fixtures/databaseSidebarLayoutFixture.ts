import type { DbTable } from "../../src/lib/types"
import {
  useDbStore,
  type DbConnection,
  type DbHistoryEntry,
  type SavedDbConnection
} from "../../src/state/dbStore"
import { useUiStore } from "../../src/state/uiStore"

export const DATABASE_SIDEBAR_FIXTURE_COUNTS = {
  profiles: 12,
  objects: 45,
  history: 50
} as const

/** Deterministic browser-only seed used by Playwright CLI geometry acceptance.
 * It replaces startup hydration before Database mode mounts, so no Tauri IPC or
 * persisted browser data participates in the layout proof. */
export function seedDatabaseSidebarLayoutFixture(): void {
  const saved: SavedDbConnection[] = Array.from(
    { length: DATABASE_SIDEBAR_FIXTURE_COUNTS.profiles },
    (_, index) => ({
      id: `layout-profile-${index + 1}`,
      configGeneration: 1,
      targetKey: `/tmp/layout-${index + 1}.sqlite`,
      kind: "sqlite",
      name: `layout-${String(index + 1).padStart(2, "0")}.sqlite`,
      path: `/tmp/layout-${index + 1}.sqlite`,
      credentialState: "notRequired"
    })
  )
  const active = saved[0]
  const connection: DbConnection = {
    connId: "layout-connection",
    connectionGeneration: "layout-generation" as never,
    descriptorId: active.id,
    targetKey: active.targetKey,
    kind: "sqlite",
    name: active.name,
    title: active.path ?? active.name
  }
  const objects: DbTable[] = Array.from(
    { length: DATABASE_SIDEBAR_FIXTURE_COUNTS.objects },
    (_, index) => ({
      catalog: `catalog_${Math.floor(index / 30) + 1}`,
      schema: `schema_${Math.floor(index / 10) + 1}`,
      name: `object_${String(index + 1).padStart(2, "0")}`,
      kind: index % 5 === 0 ? "view" : "table"
    })
  )
  const history: DbHistoryEntry[] = Array.from(
    { length: DATABASE_SIDEBAR_FIXTURE_COUNTS.history },
    (_, index) => ({
      sql: `SELECT ${index + 1} AS fixture_value`,
      ranAt: Date.now() - index * 1000,
      ok: index % 9 !== 0,
      error: index % 9 === 0 ? "queryFailed" : undefined,
      elapsedMs: index + 1
    })
  )

  useDbStore.setState({
    initializeProfiles: async () => {},
    saved,
    connections: [connection],
    activeDescriptorId: active.id,
    activeConnId: connection.connId,
    liveMru: [active.id],
    sessions: Object.fromEntries(saved.map((profile) => [
      profile.id,
      {
        descriptorId: profile.id,
        connId: profile.id === active.id ? connection.connId : null,
        status: profile.id === active.id ? "connected" as const : "disconnected" as const,
        error: null
      }
    ])),
    tableBuckets: { [active.id]: objects },
    tableErrors: { [active.id]: null },
    columnBuckets: {},
    columnErrors: {},
    tables: { [connection.connId]: objects },
    historyBuckets: { [active.id]: history },
    recovery: [],
    profilesLoaded: true,
    profileError: null
  })
  useUiStore.getState().setMode("database")
}

export default seedDatabaseSidebarLayoutFixture
