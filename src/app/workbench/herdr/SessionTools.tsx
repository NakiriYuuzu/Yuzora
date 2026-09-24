import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { FieldGroup } from "@/components/ui/field"
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from "@/components/ui/card"
import { parseRuntimeScope, sessionScope } from "@/lib/herdrProvider"
import { LOCAL_HOST_ID, runtimeKey } from "@/lib/runtimeIdentity"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useHerdrStore } from "@/state/herdrStore"
import { TextField } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

export function SessionTools({ sessionName, operation, onSelect }: { sessionName: string; operation: HerdrOperation; onSelect: (scope: string) => void }) {
  const { t } = useTranslation("herdrTools")
  const [name, setName] = useState("")
  const hostId = parseRuntimeScope(sessionName).hostId
  const sessions = useHerdrStore(s => s.sessions)
  const sameHost = sessions.filter(s => parseRuntimeScope(sessionScope(s)!).hostId === hostId)
  return <FieldGroup>
    <Card><CardHeader><CardTitle>{t("createSession")}</CardTitle><CardDescription>{t("sessionHostHint")}</CardDescription></CardHeader><CardContent><TextField label={t("name")} value={name} onChange={setName} pattern="[A-Za-z0-9_.-]{1,64}" /></CardContent><CardFooter>
      <Button disabled={operation.busy || !/^[A-Za-z0-9_.-]{1,64}$/.test(name) || [".", ".."].includes(name)} onClick={async () => {
        const scope = hostId === LOCAL_HOST_ID ? name : runtimeKey({ hostId, sessionName: name })
        if (await operation.run({ method: "session.start", params: {} }, scope)) { setName(""); onSelect(scope) }
      }}>{t("createAndStart")}</Button>
    </CardFooter></Card>
    {sameHost.map(session => <Card key={sessionScope(session)} size="sm"><CardHeader><CardTitle>{session.name}</CardTitle><CardDescription>{t(session.running ? "running" : "stopped")}</CardDescription></CardHeader><CardFooter className="flex flex-wrap gap-2">
      <Button variant="outline" disabled={operation.busy} onClick={async () => {
        const scope = sessionScope(session)!
        if (session.running || await operation.run({ method: "session.start", params: {} }, scope)) { onSelect(scope); await useHerdrStore.getState().selectSession(scope) }
      }}>{t(session.running ? "load" : "start")}</Button>
      <Button variant="destructive" disabled={operation.busy} onClick={async () => {
        const action = session.running ? "session.stop" : "session.delete"
        if (await requestAppConfirmation({ title: t(session.running ? "stopSession" : "deleteSession"), description: t(session.running ? "stopSessionWarning" : "deleteSessionWarning", { name: session.name }), destructive: true })) await operation.run({ method: action, params: {} }, sessionScope(session)!)
      }}>{t(session.running ? "stop" : "delete")}</Button>
    </CardFooter></Card>)}
  </FieldGroup>
}
