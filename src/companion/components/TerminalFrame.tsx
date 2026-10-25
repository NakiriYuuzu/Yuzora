import { useLayoutEffect, useRef, useState } from "react"
import type { Terminal } from "@xterm/xterm"

export function TerminalFrame({ terminal, mount, fit, cols, rows }: { terminal: Terminal | null; mount: (element: HTMLDivElement | null) => void; fit: boolean; cols: number; rows: number }) {
  const outer = useRef<HTMLDivElement>(null)
  const [box, setBox] = useState({ scale: 1, width: 0, height: 0 })

  useLayoutEffect(() => {
    const container = outer.current
    const screen = terminal?.element?.querySelector<HTMLElement>(".xterm-screen")
    if (!container || !screen) return
    const measure = () => {
      const width = screen.offsetWidth
      const height = screen.offsetHeight
      const scale = fit && width > 0 ? Math.min(1, container.clientWidth / width) : 1
      setBox({ scale, width, height })
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(container)
    observer.observe(screen)
    return () => observer.disconnect()
  }, [terminal, fit, cols, rows])

  return (
    <div ref={outer} className="w-full overflow-x-auto">
      <div style={{ width: box.width * box.scale, height: box.height * box.scale }}>
        <div ref={mount} style={{ transform: `scale(${box.scale})`, transformOrigin: "top left", width: box.width || undefined }} />
      </div>
    </div>
  )
}
