// SPDX-License-Identifier: Apache-2.0
// Vite 8 transforms TSX with Oxc (no @vitejs/plugin-react, Ruling 85: its
// Babel chain pulls a CC-BY-4.0 package). The automatic JSX runtime is set
// explicitly. Tauri serves the built files from dist/ and the dev server on a
// fixed port.
import { defineConfig } from "vitest/config";

export default defineConfig({
  oxc: { jsx: { runtime: "automatic" } },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", target: "es2022", sourcemap: false },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
