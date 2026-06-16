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
});
