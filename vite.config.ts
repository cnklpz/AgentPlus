import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and no screen clearing.
// src-tauri/target holds tens of thousands of build files (and stray tauri-codegen .html
// copies): crawling it for dependency entries and watching it kept the first page load
// waiting for up to minutes, a blank window in `tauri dev`.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  optimizeDeps: { entries: ["index.html"] },
  build: { target: "es2021", outDir: "dist" },
});
