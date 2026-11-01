// SPDX-License-Identifier: GPL-3.0-or-later
// Virtual devices (BRIEF §9): each has its own flash image, option bytes, SD card and settings.
// OPFS: /devices/<id>/{device.json, flash.bin, ob.bin, sd.idx, sd.dat}. IndexedDB where sync access
// handles are missing. Nothing persists if neither opens, and the UI says so.

import { MemoryOnlyStore, SlotStore, BLOCK, RECORD, type CardStore, type Persistence, type RandomAccess } from "./cardstore";

export interface BoardSettings {
  /** Device name, 1 to 8 ASCII characters. */
  name: string;
  /** FuriHalVersionRegion: 1 EU/RU, 2 US/CA/AU, 3 JP, 4 world. */
  region: number;
  /** FuriHalVersionColor: 1 black, 2 white, 3 transparent. */
  color: number;
}

export interface BatterySettings {
  percent: number;
  charging: boolean;
  usb: boolean;
}

export interface DeviceMeta {
  id: string;
  /** The label in the device list; the firmware's device name is in `board`. */
  label: string;
  legacySleep: boolean;
  board: BoardSettings;
  battery: BatterySettings;
  /** 16 hex digits. */
  uid: string;
  /** What was last installed, for display. */
  firmware: string | null;
}

export type Blob = "flash" | "ob";

export interface DeviceBackend {
  readonly kind: Persistence;
  list(): Promise<DeviceMeta[]>;
  save(meta: DeviceMeta): Promise<void>;
  /** Removes the device and everything it stores. */
  remove(id: string): Promise<void>;
  /** The device's card. At most one is open; opening another closes the previous one. */
  openCard(id: string): Promise<CardStore>;
  read(id: string, blob: Blob): Promise<Uint8Array | null>;
  write(id: string, blob: Blob, data: Uint8Array): Promise<void>;
  /** Device shown after a reload. */
  getActive(): Promise<string | null>;
  setActive(id: string): Promise<void>;
}

/** A random 64-bit ID, so that two virtual devices never look like the same one. */
export function newUid(fill: (a: Uint8Array<ArrayBuffer>) => void = (a) => crypto.getRandomValues(a)): string {
  const a = new Uint8Array(8);
  fill(a);
  return [...a].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function newDevice(label: string, id: string, uid = newUid()): DeviceMeta {
  return {
    id,
    label,
    legacySleep: true,
    board: { name: "dryflash", region: 4, color: 1 },
    battery: { percent: 80, charging: false, usb: false },
    uid,
    firmware: null,
  };
}

/** Fills fields a device saved by an older version does not have. */
export function normalize(raw: Partial<DeviceMeta> & { id: string }): DeviceMeta {
  // The folder of the version before profiles is called "default" and has no label.
  const d = newDevice(raw.label ?? (raw.id === "default" ? "Device 1" : raw.id), raw.id, raw.uid ?? newUid());
  return { ...d, ...raw, board: { ...d.board, ...raw.board }, battery: { ...d.battery, ...raw.battery } };
}

// ---- Export format: one file with a device's whole state (gzip around this layout). ----

const MAGIC = "DRYFLASH-DEVICE-1\n";

export interface DeviceExport {
  meta: DeviceMeta;
  flash: Uint8Array;
  ob: Uint8Array;
  /** Card blocks as records (4-byte LBA + 512 bytes). */
  card: Uint8Array;
}

function section(data: Uint8Array): Uint8Array {
  const out = new Uint8Array(4 + data.length);
  new DataView(out.buffer).setUint32(0, data.length, true);
  out.set(data, 4);
  return out;
}

export function encodeExport(x: DeviceExport): Uint8Array {
  const parts = [
    new TextEncoder().encode(MAGIC),
    section(new TextEncoder().encode(JSON.stringify(x.meta))),
    section(x.flash),
    section(x.ob),
    section(x.card),
  ];
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

export function decodeExport(bytes: Uint8Array): DeviceExport {
  const magic = new TextEncoder().encode(MAGIC);
  if (bytes.length < magic.length || magic.some((b, i) => bytes[i] !== b)) {
    throw new Error("not a dryflash device file");
  }
  let o = magic.length;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const next = () => {
    if (o + 4 > bytes.length) throw new Error("truncated device file");
    const n = view.getUint32(o, true);
    o += 4;
    if (o + n > bytes.length) throw new Error("truncated device file");
    const s = bytes.slice(o, o + n);
    o += n;
    return s;
  };
  const raw = JSON.parse(new TextDecoder().decode(next()));
  if (typeof raw?.id !== "string") throw new Error("bad device file");
  const flash = next();
  const ob = next();
  const card = next();
  if (card.length % RECORD !== 0) throw new Error("bad card data in device file");
  return { meta: normalize(raw), flash, ob, card };
}

async function pipe(data: Uint8Array, stream: { readable: ReadableStream; writable: WritableStream }) {
  const writer = stream.writable.getWriter();
  void writer.write(data).then(() => writer.close()).catch(() => {});
  return new Uint8Array(await new Response(stream.readable).arrayBuffer());
}

export const gzip = (d: Uint8Array) => pipe(d, new CompressionStream("gzip"));
export const gunzip = (d: Uint8Array) => pipe(d, new DecompressionStream("gzip"));

// ---- OPFS ----

type SyncFile = RandomAccess & { close(): void };

class OpfsBackend implements DeviceBackend {
  readonly kind: Persistence = "opfs";
  private open: SyncFile[] = [];
  constructor(private root: FileSystemDirectoryHandle) {}

  private dir(id: string, create = true) {
    return this.root.getDirectoryHandle(id, { create });
  }

  private closeCard() {
    for (const f of this.open) f.close();
    this.open = [];
  }

  private async readJson<T>(dir: FileSystemDirectoryHandle, name: string): Promise<T | null> {
    try {
      const f = await (await dir.getFileHandle(name)).getFile();
      return JSON.parse(await f.text()) as T;
    } catch {
      return null;
    }
  }

  private async writeFile(dir: FileSystemDirectoryHandle, name: string, data: Uint8Array) {
    const h = (await (await dir.getFileHandle(name, { create: true })).createSyncAccessHandle()) as unknown as SyncFile;
    try {
      h.truncate(0);
      h.write(data, { at: 0 });
      h.flush();
    } finally {
      h.close();
    }
  }

  async list(): Promise<DeviceMeta[]> {
    const out: DeviceMeta[] = [];
    // Directory iteration is missing from some DOM typings.
    const entries = (this.root as unknown as { entries(): AsyncIterable<[string, FileSystemHandle]> }).entries();
    for await (const [name, handle] of entries) {
      if (handle.kind !== "directory") continue;
      const meta = await this.readJson<Partial<DeviceMeta>>(handle as FileSystemDirectoryHandle, "device.json");
      // A folder with no device.json is the single card of the version before profiles.
      out.push(normalize({ ...meta, id: name }));
    }
    return out.sort((a, b) => a.label.localeCompare(b.label));
  }

  async save(meta: DeviceMeta) {
    await this.writeFile(await this.dir(meta.id), "device.json", new TextEncoder().encode(JSON.stringify(meta)));
  }

  async remove(id: string) {
    this.closeCard();
    await this.root.removeEntry(id, { recursive: true });
  }

  async openCard(id: string): Promise<CardStore> {
    this.closeCard();
    const dir = await this.dir(id);
    const handle = async (name: string) =>
      (await (await dir.getFileHandle(name, { create: true })).createSyncAccessHandle()) as unknown as SyncFile;
    const idx = await handle("sd.idx");
    this.open.push(idx);
    try {
      const dat = await handle("sd.dat");
      this.open.push(dat);
      return new SlotStore(idx, dat);
    } catch (err) {
      this.closeCard(); // a retry must not find its own lock on sd.idx
      throw err;
    }
  }

  async read(id: string, blob: Blob) {
    try {
      const f = await (await (await this.dir(id)).getFileHandle(`${blob === "flash" ? "flash" : "ob"}.bin`)).getFile();
      return new Uint8Array(await f.arrayBuffer());
    } catch {
      return null;
    }
  }

  async write(id: string, blob: Blob, data: Uint8Array) {
    await this.writeFile(await this.dir(id), `${blob}.bin`, data);
  }

  async getActive() {
    return (await this.readJson<string>(this.root, "active.json")) ?? null;
  }

  async setActive(id: string) {
    await this.writeFile(this.root, "active.json", new TextEncoder().encode(JSON.stringify(id)));
  }
}

// ---- IndexedDB ----

const req = <T>(r: IDBRequest<T>) =>
  new Promise<T>((ok, fail) => {
    r.onsuccess = () => ok(r.result);
    r.onerror = () => fail(r.error);
  });

const done = (tx: IDBTransaction) =>
  new Promise<void>((ok, fail) => {
    tx.oncomplete = () => ok();
    tx.onerror = () => fail(tx.error);
    tx.onabort = () => fail(tx.error);
  });

class IdbCard implements CardStore {
  readonly kind: Persistence = "indexeddb";
  constructor(
    private db: IDBDatabase,
    private id: string,
  ) {}

  private range() {
    return IDBKeyRange.bound([this.id, 0], [this.id, Infinity]);
  }

  async load() {
    const s = this.db.transaction("blocks").objectStore("blocks");
    const [keys, values] = await Promise.all([req(s.getAllKeys(this.range())), req(s.getAll(this.range()))]);
    const out = new Uint8Array(keys.length * RECORD);
    const view = new DataView(out.buffer);
    keys.forEach((k, i) => {
      view.setUint32(i * RECORD, (k as [string, number])[1], true);
      out.set(values[i] as Uint8Array, i * RECORD + 4);
    });
    return out;
  }

  async put(records: Uint8Array) {
    const tx = this.db.transaction("blocks", "readwrite");
    const s = tx.objectStore("blocks");
    const view = new DataView(records.buffer, records.byteOffset, records.byteLength);
    for (let o = 0; o + RECORD <= records.length; o += RECORD) {
      s.put(records.slice(o + 4, o + 4 + BLOCK), [this.id, view.getUint32(o, true)]);
    }
    await done(tx);
  }

  async clear() {
    await req(this.db.transaction("blocks", "readwrite").objectStore("blocks").delete(this.range()));
  }
}

class IdbBackend implements DeviceBackend {
  readonly kind: Persistence = "indexeddb";
  constructor(private db: IDBDatabase) {}

  async list() {
    const all = await req(this.db.transaction("meta").objectStore("meta").getAll());
    return (all as DeviceMeta[]).map((m) => normalize(m)).sort((a, b) => a.label.localeCompare(b.label));
  }

  async save(meta: DeviceMeta) {
    await req(this.db.transaction("meta", "readwrite").objectStore("meta").put(meta, meta.id));
  }

  async remove(id: string) {
    const tx = this.db.transaction(["meta", "blobs", "blocks"], "readwrite");
    tx.objectStore("meta").delete(id);
    tx.objectStore("blobs").delete(`${id}/flash`);
    tx.objectStore("blobs").delete(`${id}/ob`);
    tx.objectStore("blocks").delete(IDBKeyRange.bound([id, 0], [id, Infinity]));
    await done(tx);
  }

  async openCard(id: string) {
    return new IdbCard(this.db, id);
  }

  async read(id: string, blob: Blob) {
    const v = await req(this.db.transaction("blobs").objectStore("blobs").get(`${id}/${blob}`));
    return (v as Uint8Array | undefined) ?? null;
  }

  async write(id: string, blob: Blob, data: Uint8Array) {
    await req(this.db.transaction("blobs", "readwrite").objectStore("blobs").put(data, `${id}/${blob}`));
  }

  async getActive() {
    const v = await req(this.db.transaction("blobs").objectStore("blobs").get("active"));
    return (v as string | undefined) ?? null;
  }

  async setActive(id: string) {
    await req(this.db.transaction("blobs", "readwrite").objectStore("blobs").put(id, "active"));
  }
}

// ---- Nothing persists ----

export class MemoryBackend implements DeviceBackend {
  readonly kind: Persistence = "none";
  private metas = new Map<string, DeviceMeta>();
  private active: string | null = null;

  async list() {
    return [...this.metas.values()];
  }
  async save(meta: DeviceMeta) {
    this.metas.set(meta.id, meta);
  }
  async remove(id: string) {
    this.metas.delete(id);
  }
  async openCard() {
    return new MemoryOnlyStore();
  }
  async read() {
    return null;
  }
  async write() {}
  async getActive() {
    return this.active;
  }
  async setActive(id: string) {
    this.active = id;
  }
}

async function openIdb(): Promise<DeviceBackend> {
  const r = indexedDB.open("dryflash", 1);
  r.onupgradeneeded = () => {
    r.result.createObjectStore("meta");
    r.result.createObjectStore("blobs");
    r.result.createObjectStore("blocks");
  };
  return new IdbBackend(await req(r));
}

/**
 * Chooses the backend. Another tab holding the device's card is seen later, by `openCard`:
 * OPFS allows one sync handle per file.
 */
export async function openDevices(): Promise<DeviceBackend> {
  const hasSyncHandles =
    typeof FileSystemFileHandle !== "undefined" && "createSyncAccessHandle" in FileSystemFileHandle.prototype;
  try {
    if (hasSyncHandles) {
      const root = await navigator.storage.getDirectory();
      return new OpfsBackend(await root.getDirectoryHandle("devices", { create: true }));
    }
    return await openIdb();
  } catch {
    return new MemoryBackend();
  }
}
