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
    // Two pages: the window, and the menu-bar popover (popover.rs). The style guide is served by
    // the dev server only, so it is not an entry and never ships.
    rollupOptions: {
      input: {
        main: "index.html",
        popover: "popover.html",
      },
    },
  },
});
