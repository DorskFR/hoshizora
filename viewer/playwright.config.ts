import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e",
  use: { baseURL: "http://localhost:4319/" },
  webServer: {
    command: "npx vite --port 4319 --strictPort",
    url: "http://localhost:4319/",
    reuseExistingServer: false,
  },
});
