// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { MAX_BYTES, checkUrl, fetchFirmware, fileName, linkMustAsk, parseLink } from "./remote";

const BASE = "https://app.dryflash.workers.dev/";
const GH = "https://github.com/a/b/releases/download/v1/update.tgz";

describe("checkUrl", () => {
  it("takes https and this site's own paths", () => {
    expect(checkUrl(GH, BASE).href).toBe(GH);
    expect(checkUrl("/x.dfu", "http://localhost:5173/").href).toBe("http://localhost:5173/x.dfu");
  });

  it.each(["http://example.com/a", "file:///etc/passwd", "javascript:alert(1)", "blob:https://x/1"])(
    "refuses %s",
    (u) => {
      expect(() => checkUrl(u, BASE)).toThrow(/only https/);
    },
  );

  it("names the file from the URL", () => {
    expect(fileName(new URL(GH))).toBe("update.tgz");
    expect(fileName(new URL("https://x.example/a%20b.dfu"))).toBe("a b.dfu");
    expect(fileName(new URL("https://x.example/"))).toBe("x.example");
  });
});

describe("fetchFirmware", () => {
  it("uses the host directly when it allows it", async () => {
    const f = (async () => new Response(new Uint8Array(4))) as unknown as typeof fetch;
    const r = await fetchFirmware(GH, BASE, undefined, f);
    expect(r).toMatchObject({ name: "update.tgz", viaProxy: false });
    expect(r.bytes.byteLength).toBe(4);
  });

  it("falls back to the proxy on a CORS failure, and says so", async () => {
    const seen: string[] = [];
    const f = (async (u: string) => {
      seen.push(u);
      if (!u.startsWith("/proxy/")) throw new TypeError("Failed to fetch");
      return new Response(new Uint8Array(2));
    }) as unknown as typeof fetch;
    const progress: number[] = [];
    const r = await fetchFirmware(GH, BASE, (d) => progress.push(d), f);
    expect(r.viaProxy).toBe(true);
    expect(seen).toEqual([GH, GH, `/proxy/fetch?url=${encodeURIComponent(GH)}`]);
    expect(progress.at(-1)).toBe(2);
  });

  it("asks past the browser's cache before the proxy", async () => {
    // A cached copy made for another origin fails CORS; a fresh request does not.
    const f = (async (_u: string, init?: RequestInit) => {
      if (init?.cache !== "reload") throw new TypeError("Failed to fetch");
      return new Response(new Uint8Array(3));
    }) as unknown as typeof fetch;
    const r = await fetchFirmware(GH, BASE, undefined, f);
    expect(r).toMatchObject({ viaProxy: false });
    expect(r.bytes.byteLength).toBe(3);
  });

  it("says so when the site has no proxy", async () => {
    const f = (async (u: string) => {
      if (!u.startsWith("/proxy/")) throw new TypeError("Failed to fetch");
      return new Response("<!doctype html>", { headers: { "Content-Type": "text/html" } });
    }) as unknown as typeof fetch;
    await expect(fetchFirmware(GH, BASE, undefined, f)).rejects.toThrow(/no download proxy/);
  });

  it("reports what the proxy refuses", async () => {
    const f = (async (u: string) => {
      if (!u.startsWith("/proxy/")) throw new TypeError("Failed to fetch");
      return new Response("URL not in the allowlist", { status: 403 });
    }) as unknown as typeof fetch;
    await expect(fetchFirmware(GH, BASE, undefined, f)).rejects.toThrow(/403.*allowlist/);
  });

  it("does not hide an error from the host behind the proxy", async () => {
    const f = (async () => new Response("no", { status: 404 })) as unknown as typeof fetch;
    await expect(fetchFirmware(GH, BASE, undefined, f)).rejects.toThrow(/404/);
  });

  it("refuses too much data, stated or not", async () => {
    const stated = (async () =>
      new Response(null, { headers: { "Content-Length": String(MAX_BYTES + 1) } })) as unknown as typeof fetch;
    await expect(fetchFirmware(GH, BASE, undefined, stated)).rejects.toThrow(/64 MiB/);
  });
});

describe("links", () => {
  it("parses the parameters", () => {
    expect(parseLink("")).toBeNull();
    expect(parseLink(`?fw=${encodeURIComponent(GH)}&mode=ota&autoboot=1&device=Lab&elf=e`)).toEqual({
      fw: GH,
      elf: "e",
      ota: true,
      autoboot: true,
      device: "Lab",
      frames: false,
    });
  });

  it("never overwrites a named device's firmware without asking", () => {
    const named = parseLink("?fw=x&autoboot=1&device=Lab")!;
    expect(linkMustAsk(named, { hasFirmware: true })).toBe(true);
    expect(linkMustAsk(named, { hasFirmware: false })).toBe(false);
    expect(linkMustAsk(parseLink("?fw=x&autoboot=1")!, null)).toBe(false);
    expect(linkMustAsk(parseLink("?fw=x")!, null)).toBe(true);
  });
});
