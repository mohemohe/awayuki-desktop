import { defineConfig } from "vitest/config";

export default defineConfig({
  root: "frontend",
  test: {
    environment: "jsdom",
    restoreMocks: true,
    setupFiles: ["./src/test/setup.ts"],
  },
});
