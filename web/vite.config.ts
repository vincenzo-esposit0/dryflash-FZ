// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

// Relative base so the static build works on any host path (GitHub Pages project URL or Cloudflare Pages).
export default defineConfig({
  base: "./",
  plugins: [preact()],
});
