// SPDX-License-Identifier: GPL-3.0-or-later
// ADR-0001 spike: serves the benchmark page and collects results POSTed by browsers.
// usage: node serve.mjs <harness.wasm> <bench-fw.bin> <results.jsonl> [port]
import { createServer } from "node:http";
import { readFileSync, appendFileSync } from "node:fs";

const [wasm, fw, results, port = "8765"] = process.argv.slice(2);
const files = {
  "/": ["web/index.html", "text/html"],
  "/harness.wasm": [wasm, "application/wasm"],
  "/bench-fw.bin": [fw, "application/octet-stream"],
};
createServer((req, res) => {
  const path = new URL(req.url, "http://x").pathname;
  if (req.method === "POST" && path === "/result") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      appendFileSync(results, body.replace(/\n/g, "") + "\n");
      console.log(body);
      res.end("ok");
    });
    return;
  }
  const f = files[path];
  if (!f) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": f[1], "cache-control": "no-store" });
  res.end(readFileSync(new URL(f[0], import.meta.url)));
}).listen(Number(port), "127.0.0.1", () => console.log(`http://127.0.0.1:${port}/`));
