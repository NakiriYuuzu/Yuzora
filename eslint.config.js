import js from "@eslint/js"
import tseslint from "typescript-eslint"
import reactHooks from "eslint-plugin-react-hooks"
import reactRefresh from "eslint-plugin-react-refresh"

export default tseslint.config(
    {
        ignores: ["dist", "src-tauri", "node_modules", "docs", "spikes", "fixtures", "site-remotion"]
    },
    js.configs.recommended,
    ...tseslint.configs.recommended,
    {
        files: ["src/**/*.{ts,tsx}"],
        languageOptions: {
            ecmaVersion: 2022
        },
        plugins: {
            "react-hooks": reactHooks,
            "react-refresh": reactRefresh
        },
        rules: {
            ...reactHooks.configs["recommended-latest"].rules,
            // v7 compiler-based rules are opinionated and flag intentional patterns
            // (ref-as-cache adjusted during render, deliberate setState-in-effect).
            // Keep them visible as warnings; rules-of-hooks/exhaustive-deps stay at
            // their recommended severity.
            "react-hooks/set-state-in-effect": "warn",
            "react-hooks/refs": "warn",
            "react-hooks/use-memo": "warn",
            "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
            "no-undef": "off",
            "prefer-const": ["error", { ignoreReadBeforeAssign: true }],
            "@typescript-eslint/no-unused-vars": [
                "error",
                { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }
            ]
        }
    },
    {
        files: ["**/*.test.{ts,tsx}", "src/test/**", "src/**/fake*.{ts,tsx}"],
        rules: {
            "@typescript-eslint/no-explicit-any": "off",
            "@typescript-eslint/no-non-null-assertion": "off"
        }
    }
)
