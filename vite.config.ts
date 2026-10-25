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
  },
  build: {
    outDir: "dist",
  },
});
