/// <reference types="vitest/config" />
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig(({ mode }) => ({
  plugins: [svelte()],
  // Tauri expects a fixed port and prints its own output.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"] },
  },
  // The interface's tests (`npm run test:ui`, docs/agents/areas/testing.md):
  // the components and modules of `src/` in Node, jsdom for the page,
  // xterm.js itself, and `src/test/shell.ts` for the shell on the other
  // side of the IPC.
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
    setupFiles: ["src/test/setup.ts"],
  },
  // Svelte's browser build in the tests too, as in the app.
  resolve: mode === "test" ? { conditions: ["browser"] } : undefined,
}));
