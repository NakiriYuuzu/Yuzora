import { lazy, Suspense } from "react"
import { useHerdrToolsStore } from "@/state/herdrToolsStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"

const HerdrToolsDialog = lazy(() => import("@/app/workbench/herdr/HerdrToolsDialog"))
const HerdrNativeDialog = lazy(() => import("@/app/workbench/herdr/HerdrNativeDialog"))
export function HerdrToolsHost() {
  const selection = useHerdrToolsStore(s => s.selection)
  const native = useHerdrNativeStore(s => s.selection)
  return <Suspense fallback={null}>
    {native ? <HerdrNativeDialog key={JSON.stringify(native)} selection={native} /> : selection && <HerdrToolsDialog key={JSON.stringify(selection)} selection={selection} />}
  </Suspense>
}
