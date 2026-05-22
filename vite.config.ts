/// <reference types="vitest" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test-setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    // Pre-existing test files use a hand-rolled assertion harness instead
    // of vitest's describe/it. They never ran in CI (no test script existed
    // before). Excluding them here keeps the vitest run green; can be
    // ported on demand.
    exclude: [
      "node_modules/**",
      "src/stores/chatEvents.test.ts",
      "src/stores/diffViewer.test.ts",
      "src/stores/slashCommands.test.ts",
    ],
    css: false,
  },
});
