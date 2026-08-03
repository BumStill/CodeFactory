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
    css: false,
    // Vitest's 5s default is calibrated for an idle machine. These suites
    // render whole pages into jsdom, and CI runs them with several workers on
    // a 4-core Windows runner — the same file that takes 230ms on an idle Mac
    // took 8284ms there (run 30795455214), which is how a passing test failed.
    //
    // Note the default hid how close others already were: SessionSidebar's
    // slowest case took 8954ms in that run and still passed, because it never
    // yields to the macrotask queue and so never gave the 5s timer a chance to
    // fire. Tests are not slower or faster for this setting — it only decides
    // when we stop waiting, and 5s was declaring healthy tests dead.
    testTimeout: 20_000,
  },
});
