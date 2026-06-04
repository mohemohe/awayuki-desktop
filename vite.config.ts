import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: "frontend",
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The largest deferred chunk is the full Unicode emoji catalog. It is
    // loaded only when emoji autocomplete/picker is used and gzips to ~64 KiB.
    chunkSizeWarningLimit: 1000,
  },
});
