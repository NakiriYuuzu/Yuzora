import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { AlertCircle, RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { dbListDatabases, dbPostgresTransportChallenge } from "@/lib/ipc"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { identityOf, profileFromSaved, queryFor, useDbStore } from "@/state/dbStore"
import type { DbOpenConfig } from "@/lib/types"
import { useOverlayPresence } from "@/state/overlayStore"

export function DatabaseCatalogPicker({ descriptorId }: { descriptorId: string }) {
    const { t } = useTranslation("databaseWorkbench")
    const profile = useDbStore(state => state.saved.find(item => item.id === descriptorId))
    const connection = useDbStore(state => state.connections.find(item => item.descriptorId === descriptorId))
    const running = useDbStore(state => queryFor(state, descriptorId).running)
    const [databases, setDatabases] = useState<string[]>([])
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState(false)
    const [open, setOpen] = useState(false)
    useOverlayPresence(open)
    const mounted = useRef(true)
    const inFlight = useRef(false)
    useEffect(() => {
        mounted.current = true
        return () => { mounted.current = false }
    }, [])
    if (!profile || profile.kind === "sqlite") return null

    async function load() {
        const identity = identityOf(connection)
        if (!identity || inFlight.current || running) return
        const current = () => mounted.current && useDbStore.getState().connections.some(item => item.connId === identity.connectionId && item.connectionGeneration === identity.connectionGeneration)
        inFlight.current = true
        setBusy(true)
        setError(false)
        try {
            const names = await dbListDatabases(identity)
            if (current()) setDatabases(names)
        } catch {
            if (current()) setError(true)
        } finally {
            inFlight.current = false
            if (mounted.current) setBusy(false)
        }
    }

    async function select(database: string) {
        const target = profile && profileFromSaved(profile)?.target
        if (!target || target.kind === "sqlite" || target.database === database || inFlight.current || running) return
        const capturedConnection = connection?.connId
        const stillCurrent = () => useDbStore.getState().connections.some(item => item.descriptorId === descriptorId && item.connId === capturedConnection)
        let config: DbOpenConfig = { ...target, database, password: "" }
        let transportChallengeId: string | undefined
        inFlight.current = true
        setBusy(true)
        setError(false)
        try {
            if (target.kind === "postgres" && target.transportMode !== "verifyFull") {
                const accepted = await requestAppConfirmation({ title: t("transportTitle"), description: t("transportDescription", { database }), confirmLabel: t("connect"), cancelLabel: t("cancel"), kind: "warning" })
                if (!accepted || !stillCurrent()) return
                const challenge = await dbPostgresTransportChallenge({ ...target, database })
                transportChallengeId = challenge.challengeId
                config = { ...target, database, password: "", insecureException: target.transportMode === "insecurePlaintext" ? { host: target.host, port: target.port, user: target.user, database } : null }
            }
            if (!stillCurrent()) return
            // updateSaved records its own user intent synchronously and may disconnect
            // this profile (and unmount the picker); only a newer user choice cancels.
            const saving = useDbStore.getState().updateSaved(descriptorId, config, { transportChallengeId })
            const intent = useDbStore.getState().latestUserIntentToken
            await saving
            if (useDbStore.getState().latestUserIntentToken !== intent) return
            const outcome = await useDbStore.getState().openOrReconnectSavedConnection(descriptorId)
            if (outcome.outcome === "error") throw outcome.error
        } catch {
            if (mounted.current) setError(true)
        } finally {
            inFlight.current = false
            if (mounted.current) setBusy(false)
        }
    }

    return <div className="flex min-w-0 shrink items-center gap-0.5">
        <Select value={profile.database || ""} disabled={busy || running || !connection} onOpenChange={open => { setOpen(open); if (open) void load() }} onValueChange={value => void select(value)}>
            <SelectTrigger size="sm" className="w-[184px] max-w-full min-w-0 font-mono text-[12px]" aria-label={t("chooseDatabase")}><SelectValue placeholder={busy ? t("loading") : t("chooseDatabase")} /></SelectTrigger>
            <SelectContent><SelectGroup>
                {[...new Set([...(profile.database ? [profile.database] : []), ...databases])].map(name => <SelectItem key={name} value={name}>{name}</SelectItem>)}
                {databases.length === 0 && !profile.database && <SelectItem value="__loading__" disabled>{busy ? t("loading") : t("noDatabases")}</SelectItem>}
            </SelectGroup></SelectContent>
        </Select>
        <Button variant="ghost" size="icon-sm" disabled={busy || running || !connection} onClick={() => void load()} aria-label={t("refreshDatabases")}><RefreshCw data-icon="inline-start" /></Button>
        {error && <span role="alert" title={t("catalogFailed")} className="flex min-w-0 items-center gap-1 text-[11.5px] text-(--destructive)"><AlertCircle className="size-3.5 shrink-0" aria-hidden="true" /><span className="truncate">{t("catalogFailedShort")}</span><span className="sr-only">{t("catalogFailed")}</span></span>}
    </div>
}
