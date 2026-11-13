import js from "@eslint/js"

// The website's scripts, its page generator and the tests at the root: the
// JavaScript nothing else checks before Pages publishes it. app/ has its own
// checks, and site/en is generated.
export default [
    { ignores: ["app", "docs", "specs", "node_modules", "output", "site/en"] },
    js.configs.recommended,
]
