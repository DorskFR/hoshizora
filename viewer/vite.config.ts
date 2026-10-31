import { defineConfig } from "vitest/config";

export default defineConfig(() => {
  const api = process.env.HOSHIZORA_API ?? "http://127.0.0.1:7317";
  return {
    base: "./",
    build: {
      outDir: "dist",
      target: "es2022",
      chunkSizeWarningLimit: 1500,
    },
    server: {
      port: 5173,
      proxy: {
        "/api": api,
        "/v1": api,
      },
    },
    test: {
      include: ["tests/**/*.test.ts"],
    },
  };
});
