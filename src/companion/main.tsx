import { createRoot } from "react-dom/client"
import { App } from "./App"
import { boot } from "./boot"
import { registerServiceWorker } from "./serviceWorker"
import "./companion.css"

// Follow the system appearance; the shared tokens switch on `.dark`.
const dark = window.matchMedia("(prefers-color-scheme: dark)")
const applyTheme = () => document.documentElement.classList.toggle("dark", dark.matches)
applyTheme()
dark.addEventListener("change", applyTheme)

boot()
registerServiceWorker()
createRoot(document.getElementById("root")!).render(<App />)
