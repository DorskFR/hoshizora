import { defineConfig } from "vitest/config";

export default defineConfig({
  base: "./",
  build: {
    outDir: "dist",
    target: "es2022",
    chunkSizeWarningLimit: 1500,
  },
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:7317",
      "/v1": "http://127.0.0.1:7317",
    },
  },
  test: {
    include: ["tests/**/*.test.ts"],
  },
});
