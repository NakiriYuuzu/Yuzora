import { defineConfig } from "vite"
import { existsSync, readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const __dirname = dirname(fileURLToPath(import.meta.url))
const fixturesDir = resolve(__dirname, "..", "..", "fixtures", "out")

export default defineConfig({
    root: __dirname,
    server: {
        port: 5299,
        strictPort: true,
    },
    plugins: [
        {
            name: "fixture-server",
            configureServer(server) {
                server.middlewares.use("/fixture", (req, res) => {
                    const name = decodeURIComponent(((req.url ?? "").split("?")[0] ?? "").replace(/^\//, ""))
                    const filePath = resolve(fixturesDir, name)
                    if (!filePath.startsWith(fixturesDir) || !existsSync(filePath)) {
                        res.statusCode = 404
                        res.end("not found")
                        return
                    }
                    res.setHeader("Content-Type", "text/plain; charset=utf-8")
                    res.end(readFileSync(filePath))
                })
            },
        },
    ],
})
