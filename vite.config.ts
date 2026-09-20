import { defineConfig } from "vite";

// Tauri drives this dev server; the fixed port is what tauri.conf.json's devUrl points at.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "safari15",
    sourcemap: false,
  },
});
