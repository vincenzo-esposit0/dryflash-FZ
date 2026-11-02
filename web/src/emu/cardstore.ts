// SPDX-License-Identifier: GPL-3.0-or-later
// Persistent storage of the virtual card's blocks (ADR-0013). The core owns the card; this keeps
// the blocks it reports as written. Records: 4-byte little-endian LBA + 512 bytes of data.

export const BLOCK = 512;
export const RECORD = 4 + BLOCK;
/** More than any card here can hold. */
const MAX_FILE_BYTES = 2 ** 40;

export type Persistence = "opfs" | "indexeddb" | "none";

export interface CardStore {
  readonly kind: Persistence;
  /** All saved blocks as records. */
  load(): Promise<Uint8Array>;
  /** Saves records, replacing blocks with the same LBA. */
  put(records: Uint8Array): Promise<void>;
  clear(): Promise<void>;
}

/** The part of FileSystemSyncAccessHandle the store uses (a fake in tests). */
export interface RandomAccess {
  getSize(): number;
  read(buffer: Uint8Array, options: { at: number }): number;
  write(buffer: Uint8Array, options: { at: number }): number;
  truncate(size: number): void;
  flush(): void;
}

/**
 * Two files: `dat` holds blocks in slots of 512 bytes, `idx` the LBA of each slot. A new block's
 * data is written before its index entry, so an interrupted write never maps an LBA to garbage.
 */
export class SlotStore implements CardStore {
  readonly kind: Persistence = "opfs";
  private slots = new Map<number, number>();
  private count = 0;

  constructor(
    private idx: RandomAccess,
    private dat: RandomAccess,
  ) {
    // Seen in Chromium for a moment around a page's start: getSize() returning 2^64, an unsigned -1. Taken at its word it
    // makes an impossible allocation and the page never starts. Refusing lets openCardRetrying try again, like a held lock.
    for (const f of [idx, dat]) {
      const size = f.getSize();
      if (!(size >= 0 && size <= MAX_FILE_BYTES)) throw new Error(`the card's file reports a size of ${size}`);
    }
  }

  async load(): Promise<Uint8Array> {
    const n = Math.min(Math.floor(this.idx.getSize() / 4), Math.floor(this.dat.getSize() / BLOCK));
    const idx = new Uint8Array(n * 4);
    const dat = new Uint8Array(n * BLOCK);
    this.idx.read(idx, { at: 0 });
    this.dat.read(dat, { at: 0 });
    const view = new DataView(idx.buffer);
    this.slots.clear();
    this.count = n;
    for (let s = 0; s < n; s++) this.slots.set(view.getUint32(s * 4, true), s);
    // Slots are reused, so each LBA appears once; a fresh interrupted append has no index entry.
    const out = new Uint8Array(this.slots.size * RECORD);
    let o = 0;
    for (const [lba, s] of this.slots) {
      new DataView(out.buffer).setUint32(o, lba, true);
      out.set(dat.subarray(s * BLOCK, (s + 1) * BLOCK), o + 4);
      o += RECORD;
    }
    return out;
  }

  async put(records: Uint8Array): Promise<void> {
    const view = new DataView(records.buffer, records.byteOffset, records.byteLength);
    for (let o = 0; o + RECORD <= records.length; o += RECORD) {
      const lba = view.getUint32(o, true);
      const data = records.subarray(o + 4, o + RECORD);
      const known = this.slots.get(lba);
      if (known !== undefined) {
        this.dat.write(data, { at: known * BLOCK });
      } else {
        const s = this.count++;
        this.dat.write(data, { at: s * BLOCK });
        this.idx.write(records.subarray(o, o + 4), { at: s * 4 });
        this.slots.set(lba, s);
      }
    }
    this.dat.flush();
    this.idx.flush();
  }

  async clear(): Promise<void> {
    this.idx.truncate(0);
    this.dat.truncate(0);
    this.idx.flush();
    this.dat.flush();
    this.slots.clear();
    this.count = 0;
  }
}

/** Nothing persists: the card lasts as long as the page. The UI says so. */
export class MemoryOnlyStore implements CardStore {
  readonly kind: Persistence = "none";
  async load(): Promise<Uint8Array> {
    return new Uint8Array(0);
  }
  async put(): Promise<void> {}
  async clear(): Promise<void> {}
}
