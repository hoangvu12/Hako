import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// @tauri-apps/cli sets this when running on a physical/remote device.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
  // Prevent Vite from clobbering Rust compiler errors in the terminal.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // Tauri owns the Rust side; don't watch it from Vite.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // Expose TAURI_* env vars to the client.
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // The app only ever runs inside WebView2 (evergreen Chromium), so target a
    // modern baseline instead of Vite's broad browser matrix — no legacy
    // transpilation or polyfills shipped, smaller bundle + faster parse/startup.
    target: "chrome110",
    minify: "esbuild",
    // No sourcemaps in the shipped bundle (they're dead weight inside WebView2).
    sourcemap: false,
    rollupOptions: {
      // Two windows, two entry HTMLs: the main app (index.html) and the
      // lightweight auto-update splash (updater.html). Keeping the splash a
      // separate entry means it doesn't pull in the router/query/app bundle, so
      // it paints instantly on launch.
      input: {
        main: path.resolve(__dirname, "index.html"),
        updater: path.resolve(__dirname, "updater.html"),
      },
    },
  },
});
