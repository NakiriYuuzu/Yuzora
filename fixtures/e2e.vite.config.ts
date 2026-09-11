import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"
import path from "node:path"

// A fixed production build keeps GUI acceptance independent of dev-server HMR.
const root = path.resolve(import.meta.dirname, "..")
export default defineConfig({
  root,
  base: "./",
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.join(root, "src") } },
  build: {
    outDir: path.join(root, "output/e2e-preview"),
    emptyOutDir: true,
    rolldownOptions: {
      input: [
        "src/demo/index.html",
        "src/demo/long-session-e2e-git.html",
        "fixtures/git-graph-wide.html",
        "fixtures/markdown-reading.html",
        "fixtures/syntax-highlight-e2e.html",
      ].map(file => path.join(root, file)),
    },
  },
  preview: { host: "127.0.0.1", port: 4176, strictPort: true },
})
