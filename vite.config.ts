import path from "path"
import { readFileSync } from "fs"
import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"

const host = process.env.TAURI_DEV_HOST

// Read version from package.json at config-load time so the Settings
// UI can show the running app version without duplicating the string.
const pkgJson = JSON.parse(readFileSync(path.resolve(__dirname, "package.json"), "utf-8"))

// `vite build --mode web` (npm run build:web) produces the self-hosted
// browser build: every @tauri-apps/* module is swapped for a small shim in
// src/web-shims that talks to the axum server in ../server instead of IPC.
const TAURI_WEB_SHIMS: Record<string, string> = {
  "@tauri-apps/api/core": "core.ts",
  "@tauri-apps/api/event": "event.ts",
  "@tauri-apps/api/window": "window.ts",
  "@tauri-apps/plugin-store": "plugin-store.ts",
  "@tauri-apps/plugin-dialog": "plugin-dialog.ts",
  "@tauri-apps/plugin-opener": "plugin-opener.ts",
  "@tauri-apps/plugin-autostart": "plugin-autostart.ts",
  "@tauri-apps/plugin-http": "plugin-http.ts",
}

function webShimAliases(mode: string) {
  if (mode !== "web") return []
  return Object.entries(TAURI_WEB_SHIMS).map(([find, file]) => ({
    find: new RegExp(`^${find.replace(/[/.]/g, "\\$&")}$`),
    replacement: path.resolve(__dirname, "./src/web-shims", file),
  }))
}

// https://vitejs.dev/config/
export default defineConfig(async ({ mode }) => ({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: [
      ...webShimAliases(mode),
      { find: "@", replacement: path.resolve(__dirname, "./src") },
    ],
  },

  define: {
    __APP_VERSION__: JSON.stringify(pkgJson.version),
    __LLM_WIKI_WEB__: JSON.stringify(mode === "web"),
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  test: {
    environment: "node",
    // Loads .env.test.local into process.env for real-LLM tests.
    // The loader itself is a no-op if the file is absent, so this is
    // safe to keep on for every test run.
    setupFiles: ["./src/test-helpers/load-test-env.ts"],
  },
}))
