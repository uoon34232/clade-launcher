import path from "node:path";
import process from "node:process";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // Pre-bundle UI primitives imported dynamically by lazy modal chunks so Vite
  // never triggers an on-demand re-optimization race when opening dialogs in dev.
  optimizeDeps: {
    include: [
      "@base-ui/react/button",
      "@base-ui/react/dialog",
      "@base-ui/react/menu",
      "@base-ui/react/select",
      "@base-ui/react/tooltip",
      "lucide-react",
    ],
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}", "scripts/**/*.test.mjs"],
    setupFiles: ["src/test/setup.ts"],
  },
}));
