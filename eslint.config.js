import js from "@eslint/js"

// The website's scripts, its page generator and the tests at the root: the
// JavaScript nothing else checks before Pages publishes it. app/ has its own
// checks, site-remotion its own lint, and site/demo and site/en are builds.
export default [
    { ignores: ["app", "docs", "specs", "node_modules", "output", "site-remotion", "site/demo", "site/en"] },
    js.configs.recommended,
]
