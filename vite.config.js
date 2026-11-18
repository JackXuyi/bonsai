import { defineConfig } from "vite";

// Tauri expects a fixed port (see devUrl in src-tauri/tauri.conf.json).
export default defineConfig({
  root: "ui",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "safari15",
  },
});
