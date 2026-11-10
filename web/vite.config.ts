// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

// Relative base so the static build works on any host path (GitHub Pages project URL or Cloudflare Pages).
export default defineConfig({
  base: "./",
  plugins: [preact()],
  // Nothing inlined: the CSP has no `data:` for fonts.
  build: { assetsInlineLimit: 0 },
  // The emulator worker is an ES module (top-level await for the wasm init).
  worker: { format: "es" },
  // Downloads from hosts without CORS go through the Worker (ADR-0002): run it with
  // `npx wrangler dev --config proxy/wrangler.jsonc` to use them from the dev server.
  server: { proxy: { "/proxy": "http://localhost:8787" } },
});
