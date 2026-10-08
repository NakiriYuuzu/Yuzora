import { lazy, Suspense } from "react"
import { useHerdrToolsStore } from "@/state/herdrToolsStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { useMachinesInteractiveStore } from "@/state/machinesInteractiveStore"
import { useOverlayPresence } from "@/state/overlayStore"

const HerdrToolsDialog = lazy(() => import("@/app/workbench/herdr/HerdrToolsDialog"))
const HerdrNativeDialog = lazy(() => import("@/app/workbench/herdr/HerdrNativeDialog"))
const MachineInteractiveDialog = lazy(() => import("@/app/workbench/machines/MachineInteractiveDialog"))
export function HerdrToolsHost() {
  const selection = useHerdrToolsStore(s => s.selection)
  const native = useHerdrNativeStore(s => s.selection)
  const machine = useMachinesInteractiveStore(s => s.selection)
  // Native child webviews (Browser preview) paint above DOM; hide them under these dialogs.
  useOverlayPresence(Boolean(selection || native || machine))
  return <Suspense fallback={null}>
    {machine ? <MachineInteractiveDialog key={JSON.stringify(machine)} selection={machine} />
      : native ? <HerdrNativeDialog key={JSON.stringify(native)} selection={native} /> : selection && <HerdrToolsDialog key={JSON.stringify(selection)} selection={selection} />}
  </Suspense>
}
