import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  // Generated / non-source paths ESLint should never touch.
  {
    ignores: [
      "dist",
      "node_modules",
      "src-tauri/target",
      "src-tauri/gen",
      "src-tauri/vendor",
      "src-tauri/ffmpeg",
    ],
  },

  // Base JS + TypeScript recommended rules (non-type-checked: fast, no
  // tsconfig project service required).
  js.configs.recommended,
  ...tseslint.configs.recommended,

  // Application source: browser globals + React rules.
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      // Hooks correctness + React Compiler safety (project uses the compiler).
      ...reactHooks.configs["recommended-latest"].rules,
      // Informational: the compiler reports it skipped optimizing a component
      // because a third-party lib (e.g. @tanstack/react-virtual) is incompatible.
      // Not actionable in our code, so don't surface it as a lint problem.
      "react-hooks/incompatible-library": "off",
      "react-refresh/only-export-components": [
        "warn",
        // `Route` is TanStack Router's required per-file export; it lives next to
        // the route component by design and shouldn't trip the HMR check.
        { allowConstantExport: true, allowExportNames: ["Route"] },
      ],
      // Allow intentionally-unused identifiers prefixed with `_`.
      "@typescript-eslint/no-unused-vars": [
        "warn",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
        },
      ],
    },
  },

  // Node-side tooling / config files.
  {
    files: ["*.{js,ts}", "scripts/**/*.{js,ts}"],
    languageOptions: {
      globals: globals.node,
    },
  },

  // Entry points render to the DOM and legitimately don't export components —
  // Fast Refresh doesn't apply to them.
  {
    files: ["**/main.tsx"],
    rules: { "react-refresh/only-export-components": "off" },
  },

  // Must stay last: turns off stylistic rules that would fight Prettier.
  prettier,
);
