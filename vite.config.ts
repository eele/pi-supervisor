import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a specific dev URL and host setup.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig({
  base: "./",
  plugins: [svelte()],
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    watch: {
      // src-tauri is not a frontend source: cargo rewriting its build
      // artifacts (locked .exe) crashes the watcher with EBUSY mid-`tauri dev`.
      ignored: ["**/node_modules/**", "**/.git/**", "**/src-tauri/**"],
    },
  },
  build: {
    outDir: "dist",
  },
});
