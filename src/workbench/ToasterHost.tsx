import { useEffect, useState } from "react"
import { Toaster } from "@/components/ui/sonner"

function documentTheme(): "light" | "dark" {
  return document.documentElement.classList.contains("dark") ? "dark" : "light"
}

/** App-wide toast outlet. Follows the resolved app theme (the `dark` class on <html>), not the OS preference. */
export function ToasterHost() {
  const [theme, setTheme] = useState(documentTheme)
  useEffect(() => {
    const observer = new MutationObserver(() => setTheme(documentTheme()))
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] })
    return () => observer.disconnect()
  }, [])
  return <Toaster theme={theme} closeButton position="bottom-right" />
}
