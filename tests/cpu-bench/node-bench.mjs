// SPDX-License-Identifier: GPL-3.0-or-later
// ADR-0001 spike: runs the wasm interpreter on the benchmark image under Node (V8).
// usage: node node-bench.mjs <harness.wasm> <bench-fw.bin> [rounds] [repeats]
import { readFileSync } from "node:fs";

const [wasmPath, fwPath, rounds = "200", repeats = "5"] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath));
const x = instance.exports;
const fw = readFileSync(fwPath);
const ptr = x.bench_alloc(fw.length);
new Uint8Array(x.memory.buffer, ptr, fw.length).set(fw);

const runs = [];
for (let i = 0; i < Number(repeats); i++) {
  const t0 = performance.now();
  const n = x.bench_run(ptr, fw.length, Number(rounds));
  const ms = performance.now() - t0;
  if (n < 0) throw new Error(`benchmark did not reach BKPT #0 (stop pc 0x${x.bench_stop_pc().toString(16)})`);
  runs.push({ n, ms, mips: n / ms / 1e3 });
}
const results = Array.from({ length: 9 }, (_, i) => (x.bench_result(i) >>> 0).toString(16).padStart(8, "0"));
const best = Math.max(...runs.map((r) => r.mips));
console.log(JSON.stringify({ engine: `node ${process.version}`, rounds: Number(rounds), instret: runs[0].n, mips: runs.map((r) => +r.mips.toFixed(1)), best: +best.toFixed(1), results }));
