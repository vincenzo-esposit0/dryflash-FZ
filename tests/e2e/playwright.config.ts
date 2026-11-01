// SPDX-License-Identifier: GPL-3.0-or-later
// The production build, on the real Worker (wrangler dev): the same assets, headers and proxy as the
// live site. Build first: scripts/build-wasm.sh && npm run build.
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  timeout: 90_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:8787",
    locale: "en-US",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "npx wrangler dev --config ../../proxy/wrangler.jsonc --port 8787",
    url: "http://localhost:8787",
    reuseExistingServer: true,
    timeout: 60_000,
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
});
