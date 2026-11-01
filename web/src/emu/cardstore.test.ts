// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { BLOCK, RECORD, SlotStore, type RandomAccess } from "./cardstore";

class MemFile implements RandomAccess {
  bytes = new Uint8Array(0);
  getSize() {
    return this.bytes.length;
  }
  read(b: Uint8Array, { at }: { at: number }) {
    b.set(this.bytes.subarray(at, at + b.length));
    return b.length;
  }
  write(b: Uint8Array, { at }: { at: number }) {
    if (at + b.length > this.bytes.length) {
      const n = new Uint8Array(at + b.length);
      n.set(this.bytes);
      this.bytes = n;
    }
    this.bytes.set(b, at);
    return b.length;
  }
  truncate(size: number) {
    this.bytes = this.bytes.slice(0, size);
  }
  flush() {}
}

function records(...blocks: [number, number][]) {
  const out = new Uint8Array(blocks.length * RECORD);
  const view = new DataView(out.buffer);
  blocks.forEach(([lba, fill], i) => {
    view.setUint32(i * RECORD, lba, true);
    out.fill(fill, i * RECORD + 4, (i + 1) * RECORD);
  });
  return out;
}

function parse(r: Uint8Array) {
  const view = new DataView(r.buffer);
  const out = new Map<number, number>();
  for (let o = 0; o < r.length; o += RECORD) out.set(view.getUint32(o, true), r[o + 4]);
  return out;
}

describe("SlotStore", () => {
  it("survives a reopen, overwriting blocks in place", async () => {
    const [idx, dat] = [new MemFile(), new MemFile()];
    const s = new SlotStore(idx, dat);
    await s.load();
    await s.put(records([7, 0xaa], [2_000_000, 0xbb]));
    await s.put(records([7, 0xcc]));
    expect(dat.getSize()).toBe(2 * BLOCK);

    const reopened = new SlotStore(idx, dat);
    expect(parse(await reopened.load())).toEqual(
      new Map([
        [7, 0xcc],
        [2_000_000, 0xbb],
      ]),
    );
    await reopened.put(records([8, 0xdd]));
    expect(parse(await new SlotStore(idx, dat).load()).get(8)).toBe(0xdd);
  });

  it("ignores a data slot whose index entry was never written", async () => {
    const [idx, dat] = [new MemFile(), new MemFile()];
    const s = new SlotStore(idx, dat);
    await s.load();
    await s.put(records([1, 0x11]));
    dat.write(new Uint8Array(BLOCK).fill(0xee), { at: BLOCK }); // interrupted append
    expect(parse(await new SlotStore(idx, dat).load())).toEqual(new Map([[1, 0x11]]));
  });

  it("clears everything", async () => {
    const [idx, dat] = [new MemFile(), new MemFile()];
    const s = new SlotStore(idx, dat);
    await s.put(records([1, 1]));
    await s.clear();
    expect((await new SlotStore(idx, dat).load()).length).toBe(0);
  });
});
