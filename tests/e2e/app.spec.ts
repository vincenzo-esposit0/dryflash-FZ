// SPDX-License-Identifier: GPL-3.0-or-later
// Needs the pinned firmware: node scripts/fetch-firmware.mjs official
import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const TGZ = resolve(__dirname, "../../firmware/cache/official/flipper-z-f7-update-1.5.1.tgz");

/**
 * Every test fails on an uncaught error, a console error or a CSP violation. One exception: WebKit reports
 * `style-src-attr` for its own internals of a `<select>`, with no effect on rendering, and the event does not
 * always say which element. So WebKit is forgiven that directive alone: inline styles of ours would fail on
 * Chromium and Firefox, which stay strict. The CSP is not relaxed.
 */
const test = base.extend<{ errorWatch: void }>({
  errorWatch: [
    async ({ page, browserName }, use) => {
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(`uncaught: ${e.message}`));
      page.on("console", (m) => {
        const csp = /Refused to apply a stylesheet/.test(m.text());
        if (m.type() === "error" && !(csp && browserName === "webkit")) errors.push(m.text());
      });
      await page.addInitScript(() => {
        const w = window as unknown as { __violations: string[] };
        w.__violations = [];
        addEventListener("securitypolicyviolation", (e) => {
          const element = (e.target as Element | null)?.tagName;
          w.__violations.push(`${e.violatedDirective} on ${element} from ${e.blockedURI}`);
        });
      });
      await use();
      const seen = await page
        .evaluate(() => (window as unknown as { __violations?: string[] }).__violations ?? [])
        .catch(() => [] as string[]);
      const forgiven = (v: string) => browserName === "webkit" && v.startsWith("style-src-attr ");
      expect([...errors, ...seen.filter((v) => !forgiven(v))]).toEqual([]);
    },
    { auto: true },
  ],
});

// Without the watcher above: this failure is the point. Worker exceptions never reach Playwright's `pageerror`,
// so the watcher only sees them through what the page does here.
base("an exception in the worker reaches the page and its console", async ({ page, context }) => {
  const errors: string[] = [];
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  await context.route("**/*.wasm", (route) => route.abort());
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText("Error:");
  expect(errors.filter((e) => e.startsWith("dryflash worker:"))).toHaveLength(1);
});

async function install(page: Page) {
  await page.goto("/");
  await expect(page.getByText("Drop a firmware")).toBeVisible();
  await page.locator(".drop input").setInputFiles(TGZ);
  await expect(page.locator(".meter")).toContainText("Running");
}

const screen = (page: Page) => page.locator("canvas.lcd").evaluate((c: HTMLCanvasElement) => c.toDataURL());
const darkPixels = (page: Page) =>
  page.locator("canvas.lcd").evaluate((c: HTMLCanvasElement) => {
    const d = c.getContext("2d")!.getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    for (let i = 0; i < d.length; i += 4) if (d[i] < 64) n++;
    return n;
  });

async function log(page: Page) {
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  return page.locator(".log");
}

/** Waits for the firmware to show something, then for the popup of a device without factory keys. */
async function bootToPopup(page: Page) {
  await expect.poll(() => darkPixels(page), { timeout: 30_000 }).toBeGreaterThan(300);
  await expect(await log(page)).toContainText("Secure Enclave verification failed", { timeout: 30_000 });
}

async function expectScreenChanges(page: Page, before: string) {
  await expect.poll(() => screen(page), { timeout: 15_000 }).not.toBe(before);
}

test.describe("with a firmware", () => {
  test.skip(!existsSync(TGZ), "needs the pinned firmware");

  test("a package installs, boots and obeys the keyboard", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    await expect(page.locator(".log")).toContainText("card mounted");
    const popup = await screen(page);
    await page.locator("footer p").first().click();
    await page.keyboard.press("Escape");
    await expectScreenChanges(page, popup);
    await page.waitForTimeout(500);
    const desktop = await screen(page);
    await page.keyboard.press("Enter");
    await expectScreenChanges(page, desktop);
  });

  test("the drawn keys work with the mouse", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    const popup = await screen(page);
    await page.locator(".key.back").click();
    await expectScreenChanges(page, popup);
    await page.waitForTimeout(500);
    const desktop = await screen(page);
    await page.locator(".key.ok").click();
    await expectScreenChanges(page, desktop);
    await page.waitForTimeout(1000);
    const menu = await screen(page);
    await page.locator(".key.down").click();
    await expectScreenChanges(page, menu);
  });

  test("a firmware from the list downloads, says so, and boots", async ({ page }) => {
    const url = "https://update.flipperzero.one/builds/firmware/1.5.1/flipper-z-f7-update-1.5.1.tgz";
    await page.route(url, (route) =>
      route.fulfill({ path: TGZ, headers: { "Access-Control-Allow-Origin": "*", "Content-Type": "application/gzip" } }),
    );
    await page.goto("/");
    await expect(page.locator(".fw")).toHaveCount(4);
    await page.locator(".fw", { hasText: "Official" }).click();
    await expect(page.locator(".meter")).toContainText("Running", { timeout: 30_000 });
    await expect(page.getByText(`loaded from ${url}`)).toBeVisible();
    await expect(page.locator(".start")).toHaveCount(0);
    await expect.poll(() => darkPixels(page), { timeout: 30_000 }).toBeGreaterThan(300);
    await expect(page.locator(".progress")).toHaveCount(0);
  });

  test("switching language leaves the running firmware alone, and is remembered", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    const seconds = async () => Number(/(\d+\.\d) s/.exec((await page.locator(".meter").textContent()) ?? "")![1]);
    const before = await seconds();
    await page.getByRole("button", { name: /IT \/ EN/ }).click();
    await expect(page.locator("html")).toHaveAttribute("lang", "it");
    await expect(page.getByRole("tab", { name: "Diagnostica" })).toBeVisible();
    await expect(page.locator(".meter")).toContainText("In esecuzione");
    expect(await seconds()).toBeGreaterThanOrEqual(before);
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("lang", "it");
    await page.getByRole("button", { name: /IT \/ EN/ }).click();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
  });

  test("a reload resumes the device with its firmware and card", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    await page.waitForTimeout(2500); // the card is saved about once a second
    await page.reload();
    await expect(page.locator(".meter")).toContainText("Running");
    await expect(page.locator(".info dd").first()).toContainText("flipper-z-f7-update-1.5.1.tgz");
    await expect(page.locator(".info")).toContainText("Saved in this browser");
    await expect(await log(page)).toContainText("card mounted", { timeout: 30_000 });
    await expect(page.locator(".log")).not.toContainText('Failed to read "/int/.dolphin.state"');
  });

  test("files can be uploaded, downloaded and deleted, and the firmware still mounts the card", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    await page.getByRole("tab", { name: "Files" }).click();
    await expect(page.locator(".files")).toContainText("apps");
    await page.locator('.file-button:has-text("Upload") input').setInputFiles({
      name: "my long test name.sub",
      mimeType: "text/plain",
      buffer: Buffer.from("Filetype: test\n"),
    });
    const row = page.locator(".files li", { hasText: "my long test name.sub" });
    await expect(row).toBeVisible();
    const [download] = await Promise.all([page.waitForEvent("download"), row.getByRole("button", { name: "Download" }).click()]);
    // WebKit on Linux turns the spaces of a download's name into underscores; the bytes are what matter.
    expect(download.suggestedFilename().replace(/_/g, " ")).toBe("my long test name.sub");
    const saved = await (await import("node:fs/promises")).readFile(await download.path());
    expect(saved.toString()).toBe("Filetype: test\n");
    await row.getByRole("button", { name: "Delete" }).click();
    await page.locator("dialog.ask button[type=submit]").click();
    await expect(row).toHaveCount(0);
    await expect(page.locator(".meter")).toContainText("Running");
    await expect(await log(page)).toContainText("card mounted");
  });

  test("screenshot and GIF download working files", async ({ page }) => {
    await install(page);
    await expect.poll(() => darkPixels(page), { timeout: 30_000 }).toBeGreaterThan(300);
    const [png] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Screenshot" }).click()]);
    expect(png.suggestedFilename()).toMatch(/\.png$/);
    await page.getByRole("button", { name: "Record GIF" }).click();
    await page.waitForTimeout(1200);
    const [gif] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Stop recording" }).click()]);
    expect(gif.suggestedFilename()).toMatch(/\.gif$/);
    const bytes = await (await import("node:fs/promises")).readFile(await gif.path());
    expect(bytes.subarray(0, 6).toString()).toBe("GIF89a");
  });

  // Settings → LCD and Notifications: changing Volume plays C5, switching Vibro on vibrates (notification_settings_app.c).
  test("the firmware's own beep plays through WebAudio, and its buzz shows", async ({ page }) => {
    await page.addInitScript(() => {
      const w = window as unknown as { __audio: [string, number][]; __audioState?: string };
      w.__audio = [];
      const resume = AudioContext.prototype.resume;
      AudioContext.prototype.resume = async function () {
        await resume.call(this);
        w.__audioState = this.state;
      };
      const set = AudioParam.prototype.setValueAtTime;
      AudioParam.prototype.setValueAtTime = function (value: number, time: number) {
        w.__audio.push([this.defaultValue === 440 ? "hz" : "gain", value]); // an oscillator's frequency starts at 440
        return set.call(this, value, time);
      };
    });
    await install(page);
    await bootToPopup(page);
    await page.getByRole("checkbox", { name: "Sound" }).check();
    await page.locator("footer p").first().click(); // keys to the device, not the checkbox
    const step = async (key: string) => {
      await page.keyboard.press(key);
      await page.waitForTimeout(700);
    };
    // Popup, menu, Settings (Up wraps to Apps, then Settings), LCD and Notifications, down to Volume.
    for (const key of ["Escape", "Enter", "ArrowUp", "ArrowUp", "Enter", "ArrowDown", "Enter"]) await step(key);
    for (let i = 0; i < 4; i++) await step("ArrowDown");
    await step("ArrowLeft");
    const events = page.locator(".events");
    // 524.59 Hz: the firmware rounds C5 to whole timer ticks.
    await expect(events).toContainText("Speaker 525 Hz");
    await expect(events).toContainText("Speaker silent");
    const { audio, state } = await page.evaluate(() => {
      const w = window as unknown as { __audio: [string, number][]; __audioState?: string };
      return { audio: w.__audio, state: w.__audioState ?? "no answer" };
    });
    if (state === "running") {
      expect(audio.some(([p, v]) => p === "hz" && Math.abs(v - 524.59) < 0.1)).toBe(true);
      expect(audio.some(([p, v]) => p === "gain" && v > 0)).toBe(true);
    } else {
      // A machine with no audio device (Firefox on the CI runners): the page must say so, not pretend.
      console.log(`AudioContext ${state}: no sound on this machine`);
      await expect(page.getByRole("alert")).toContainText("This browser cannot play sound.");
      await expect(page.getByRole("checkbox", { name: "Sound" })).not.toBeChecked();
    }

    for (const key of ["ArrowDown", "ArrowLeft", "ArrowRight"]) await step(key);
    await expect(events).toContainText("Vibration on");
    await expect(events).toContainText("Vibration off");
    await expect(page.locator(".device.buzz")).toHaveCount(0);
  });

  test("USB charges with the firmware's red LED, and Power OFF turns the device off until USB comes back", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    const battery = page.getByRole("group", { name: "Battery" });
    const usb = battery.getByRole("checkbox", { name: "USB" });
    await expect(battery.getByRole("status")).toHaveText("On battery");
    await usb.check();
    await expect(battery.getByRole("status")).toHaveText("Charging");
    // The power service's sequence_charging: red at full, green off.
    await expect(page.locator(".events")).toContainText(/LED R[1-9]\d* G0 B0/);
    await usb.uncheck();
    await expect(battery.getByRole("status")).toHaveText("On battery");

    await page.locator("footer p").first().click(); // keys to the device, not the checkbox
    const step = async (key: string, ms = 700) => {
      await page.keyboard.press(key);
      await page.waitForTimeout(ms);
    };
    // Popup, menu, Settings, Power (fourth), Power OFF (third), the dialog's "Power Off".
    for (const key of ["Escape", "Enter", "ArrowUp", "ArrowUp", "Enter"]) await step(key);
    for (let i = 0; i < 3; i++) await step("ArrowDown", 400);
    await step("Enter");
    for (let i = 0; i < 2; i++) await step("ArrowDown", 400);
    await step("Enter");
    await step("ArrowRight");
    await expect(battery.getByRole("status")).toHaveText("Off");
    await expect(page.locator(".meter")).toContainText("Off");
    await expect.poll(() => darkPixels(page)).toBe(0);
    await expect(page.locator(".events")).toContainText("Power off");

    await usb.check(); // plugging in powers the charger's system rail back up
    await expect(battery.getByRole("status")).toHaveText("Charging");
    await expect.poll(() => darkPixels(page), { timeout: 30_000 }).toBeGreaterThan(300);
  });

  test("a saved remote's button shows what the IR LED sent, ready to save as .ir", async ({ page }) => {
    await install(page);
    await bootToPopup(page);
    await page.getByRole("tab", { name: "Files" }).click();
    await page.locator(".files li", { hasText: "infrared" }).getByRole("button", { name: "infrared" }).click();
    await expect(page.locator(".files")).toContainText("assets");
    await page.locator('.file-button:has-text("Upload") input').setInputFiles({
      name: "Test.ir",
      mimeType: "text/plain",
      buffer: Buffer.from(
        "Filetype: IR signals file\nVersion: 1\n#\nname: Power\ntype: parsed\nprotocol: NEC\naddress: 04 00 00 00\ncommand: 08 00 00 00\n",
      ),
    });
    await expect(page.locator(".files li", { hasText: "Test.ir" })).toBeVisible();
    // Changing the card restarts the device: wait for its second boot.
    await page.getByRole("tab", { name: "Diagnostics" }).click();
    await expect
      .poll(async () => ((await page.locator(".log").textContent()) ?? "").split("Secure Enclave verification failed").length - 1, {
        timeout: 30_000,
      })
      .toBe(2);

    await page.locator("footer p").first().click(); // keys to the device
    const step = async (key: string, ms = 700) => {
      await page.keyboard.press(key);
      await page.waitForTimeout(ms);
    };
    // Popup, menu, Infrared (fourth), Saved Remotes (third), past `assets` to Test.ir, Power.
    for (const key of ["Escape", "Enter"]) await step(key);
    for (let i = 0; i < 3; i++) await step("ArrowDown", 400);
    await step("Enter", 2500);
    for (let i = 0; i < 2; i++) await step("ArrowDown", 400);
    await step("Enter", 2000);
    await step("ArrowDown", 400);
    await step("Enter", 2000);
    await step("Enter", 1500);
    const sent = page.locator(".events li", { hasText: "IR sent" }).first();
    await expect(sent).toContainText(/IR sent: 38\.0 kHz carrier, duty 33 %, 67 timings, \d+\.\d ms/, { timeout: 15_000 });
    const [file] = await Promise.all([page.waitForEvent("download"), sent.getByRole("button", { name: "Save as .ir" }).click()]);
    expect(file.suggestedFilename()).toMatch(/^ir-\d+ms\.ir$/);
    const text = (await (await import("node:fs/promises")).readFile(await file.path())).toString();
    expect(text).toMatch(/^Filetype: IR signals file\nVersion: 1\n#\nname: Sent\ntype: raw\nfrequency: 3800\d\nduty_cycle: 0\.33\d+\ndata: (\d+ ){66}\d+\n$/);
  });

  test("a device exports and imports as a new one with its firmware", async ({ page }) => {
    await install(page);
    await expect.poll(() => darkPixels(page), { timeout: 30_000 }).toBeGreaterThan(300);
    await page.getByRole("tab", { name: "Backup" }).click();
    const [file] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Export" }).click()]);
    expect(file.suggestedFilename()).toMatch(/\.dryflash$/);
    await page.locator('.file-button:has-text("Import") input').setInputFiles(await file.path());
    await page.getByRole("tab", { name: "Device" }).click();
    await expect(page.locator(".tab-section select option", { hasText: "(import)" })).toHaveCount(1, { timeout: 30_000 });
    await expect(page.locator(".info dd").first()).toContainText("flipper-z-f7-update-1.5.1.tgz");
    await expect(page.locator(".meter")).toContainText("Running");
  });
});

test.describe("devices", () => {
  test("a new device can be made, is empty, and factory reset asks first", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "New device" }).click();
    await page.locator("dialog.ask input").fill("Lab");
    await page.locator("dialog.ask button[type=submit]").click();
    await expect(page.locator(".tab-section select option", { hasText: "Lab" })).toHaveCount(1);
    await expect(page.locator(".info dd").first()).toContainText("No firmware");
    await page.getByRole("button", { name: "Factory reset" }).click();
    await expect(page.locator("dialog.ask")).toBeVisible();
    await page.locator("dialog.ask").getByRole("button", { name: "Cancel" }).click();
    await expect(page.locator("dialog.ask")).toBeHidden();
  });
});

test.describe("links and the proxy", () => {
  test("a link asks first and names the source", async ({ page }) => {
    await page.goto("/?fw=https%3A%2F%2Fexample.com%2Fa.tgz");
    await expect(page.locator("dialog.ask")).toContainText("https://example.com/a.tgz");
    await expect(page.locator("dialog.ask")).toContainText("From a link");
    await page.locator("dialog.ask").getByRole("button", { name: "Cancel" }).click();
    await expect(page.locator("dialog.ask")).toBeHidden();
  });

  test("the proxy refuses what is not in its allowlist", async ({ request }) => {
    expect((await request.get("/proxy/fetch?url=https%3A%2F%2Fexample.com%2Fa")).status()).toBe(403);
    expect((await request.get("/proxy/other")).status()).toBe(404);
    expect(
      (
        await request.get(
          "/proxy/fetch?url=https%3A%2F%2Fgithub.com%2FDarkFlippers%2Funleashed-firmware%2Freleases%2Fdownload%2Fv%2Fa",
          { headers: { Origin: "https://evil.example" } },
        )
      ).status(),
    ).toBe(403);
  });

  test("the icons are served", async ({ request }) => {
    for (const icon of ["/favicon.svg", "/favicon-32.png", "/apple-touch-icon.png"]) {
      expect((await request.get(icon)).status()).toBe(200);
    }
  });

  test("only the embed page may be framed", async ({ request }) => {
    const frame = async (p: string) => (await request.get(p)).headers()["content-security-policy"];
    expect(await frame("/embed")).toContain("frame-ancestors *");
    expect(await frame("/")).toContain("frame-ancestors 'self'");
    expect(await frame("/about")).toContain("frame-ancestors 'self'");
  });
});

test.describe("pages", () => {
  test("/compat lists what the nightly run saw, and what that does not mean", async ({ page }) => {
    await page.goto("/compat");
    await expect(page.getByRole("heading", { name: "Compatibility" })).toBeVisible();
    expect(await page.locator("tbody tr").count()).toBeGreaterThanOrEqual(4);
    await expect(page.locator("tbody")).toContainText("Official");
    await expect(page.getByText("nightly CI")).toBeVisible();
    await expect(page.getByText("works on a device")).toBeVisible();
  });

  test("/about shows the fidelity table from the core", async ({ page }) => {
    await page.goto("/about");
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await expect(page.locator("tbody")).toContainText("CPU2");
    await expect(page.locator("footer")).toContainText("Not affiliated");
  });

  test("/embed shows only the device and talks to its parent", async ({ page }) => {
    await page.goto("/about");
    const ready = await page.evaluate(
      () =>
        new Promise<string>((resolve, reject) => {
          const f = document.createElement("iframe");
          f.src = "/embed";
          addEventListener("message", (e) => {
            if (e.data?.source === "dryflash" && e.data.type === "ready") resolve(e.data.version);
          });
          setTimeout(() => reject(new Error("no ready message")), 20_000);
          document.body.append(f);
        }),
    );
    expect(ready).toMatch(/\d+\.\d+/);
    const frame = page.frameLocator("iframe");
    await expect(frame.locator(".key.ok")).toBeAttached();
    await expect(frame.locator(".tabs")).toHaveCount(0);
    await expect(frame.locator("nav")).toHaveCount(0);
    await expect(frame.locator(".picker")).toHaveCount(0);
    await expect(frame.locator(".skins")).toHaveCount(0);
  });

  test("a phone-sized window has no sideways scroll, and the panel is whole-number sized", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto("/");
    await expect(page.locator(".key.ok")).toBeAttached();
    const m = await page.evaluate(() => ({
      scroll: document.documentElement.scrollWidth,
      inner: innerWidth,
      lcd: document.querySelector(".lcd")!.getBoundingClientRect().width,
    }));
    expect(m.scroll).toBeLessThanOrEqual(m.inner);
    expect(m.lcd % 128).toBe(0);
  });
});

test.describe("accessibility", () => {
  for (const path of ["/", "/compat", "/about"]) {
    test(`${path} has no WCAG A or AA violations`, async ({ page }) => {
      await page.goto(path);
      await expect(page.locator("footer")).toBeVisible();
      await page.waitForTimeout(800);
      const { violations } = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`)).toEqual([]);
    });
  }

  test("tabs work with the arrow keys", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("tab", { name: "Device" }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("tab", { name: "Update" })).toBeFocused();
    await expect(page.getByRole("tab", { name: "Update" })).toHaveAttribute("aria-selected", "true");
  });

  test("the page's language follows the text", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
  });

  test.describe("in Italian", () => {
    test.use({ locale: "it-IT" });

    test("the language and the text are Italian", async ({ page }) => {
      await page.goto("/");
      await expect(page.locator("html")).toHaveAttribute("lang", "it");
      await expect(page.getByRole("tab", { name: "Aggiorna" })).toBeVisible();
    });

    test("no WCAG A or AA violations", async ({ page }) => {
      await page.goto("/");
      await expect(page.locator("footer")).toBeVisible();
      await page.waitForTimeout(800);
      const { violations } = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(violations.map((v) => v.id)).toEqual([]);
    });
  });

  test("a drawn key with focus answers Enter, and passes arrows to the device", async ({ page }) => {
    test.skip(!existsSync(TGZ), "needs the pinned firmware");
    await install(page);
    await bootToPopup(page);
    await page.waitForTimeout(500);
    const popup = await screen(page);
    await page.locator(".key.back").focus();
    await page.keyboard.press("Enter");
    await expectScreenChanges(page, popup);
    await page.waitForTimeout(500);
    const desktop = await screen(page);
    await page.locator(".key.ok").focus();
    await page.keyboard.press("Enter");
    await expectScreenChanges(page, desktop);
    await page.waitForTimeout(1000);
    const menu = await screen(page);
    await page.keyboard.press("ArrowDown");
    await expectScreenChanges(page, menu);
  });
});

/** A DfuSe file (ST UM0391) holding `image` at the start of flash. */
function dfuse(image: Uint8Array): Buffer {
  const u32 = (v: number) => {
    const b = Buffer.alloc(4);
    b.writeUInt32LE(v);
    return b;
  };
  const name = Buffer.alloc(255);
  name.write("test");
  const target = Buffer.concat([
    Buffer.from("Target\0"),
    u32(1),
    name,
    u32(8 + image.length),
    u32(1),
    u32(0x0800_0000),
    u32(image.length),
    Buffer.from(image),
  ]);
  const body = Buffer.concat([
    Buffer.from("DfuSe\x01"),
    u32(11 + target.length),
    Buffer.from([1]),
    target,
    Buffer.from([0xff, 0xff, 0x11, 0xdf, 0x83, 0x04, 0x1a, 0x01]),
    Buffer.from("UFD\x10"),
  ]);
  let crc = 0xffffffff;
  for (const byte of body) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return Buffer.concat([body, u32(crc >>> 0)]);
}

test.describe("device skin", () => {
  const body = (page: Page) =>
    page.locator("canvas.art").evaluate((c: HTMLCanvasElement) => [...c.getContext("2d")!.getImageData(30, 40, 1, 1).data].join());

  test("follows the colour setting at once, and stays after a reload", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Drop a firmware")).toBeVisible();
    const white = await body(page);
    await expect(page.getByRole("radio", { name: "White" })).toBeChecked();
    await page.getByRole("radio", { name: "Black" }).check();
    await expect.poll(() => body(page)).not.toBe(white);
    const black = await body(page);
    await page.getByRole("radio", { name: "Transparent" }).check();
    await expect.poll(() => body(page)).not.toBe(black);
    const transparent = await body(page);
    expect(transparent).not.toBe(white);
    await page.reload();
    await expect(page.getByText("Drop a firmware")).toBeVisible();
    await expect.poll(() => body(page)).toBe(transparent);
  });
});

test.describe("crash report", () => {
  /** An undefined instruction at the reset vector, and a HardFault handler that spins. */
  function faulting(): Buffer {
    const image = new Uint8Array(0x200);
    const view = new DataView(image.buffer);
    view.setUint32(0, 0x2003_0000, true);
    view.setUint32(4, 0x0800_0101, true);
    view.setUint32(12, 0x0800_0181, true);
    view.setUint16(0x100, 0xde00, true); // udf #0
    view.setUint16(0x180, 0xe7fe, true); // b .
    return dfuse(image);
  }

  test("a fault followed by a hang stops the emulator with a report to download", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Drop a firmware")).toBeVisible();
    await page.locator(".drop input").setInputFiles({ name: "bad.dfu", mimeType: "application/octet-stream", buffer: faulting() });
    await expect(page.getByRole("alert")).toContainText("Hang: 5 s", { timeout: 30_000 });
    await page.getByRole("tab", { name: "Diagnostics" }).click();
    await expect(page.locator(".verdict")).toContainText("Could be either");
    await expect(page.locator(".report")).toContainText("UNDEFINSTR");

    const [json] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Download JSON" }).click()]);
    const report = JSON.parse(readFileSync(await json.path(), "utf8"));
    expect(report.kind).toBe("hang");
    expect(report.verdict).toBe("unclear");
    expect(report.firmware.name).toBe("bad.dfu");
    expect(report.firmware.sha256).toMatch(/^[0-9a-f]{64}$/);
    expect(report.registers.pc).toBe("0x08000100");
    expect(report.recentPcs.length).toBeGreaterThan(0);
    const [md] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Download Markdown" }).click()]);
    expect(readFileSync(await md.path(), "utf8")).toContain("# dryflash crash report");
    const [png] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Download screen" }).click()]);
    expect(readFileSync(await png.path()).subarray(1, 4).toString()).toBe("PNG");

    // Resuming is allowed: the alert goes, and the report stays to read.
    await page.getByRole("button", { name: "Resume" }).click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expect(page.locator(".report")).toBeVisible();
  });

  test("the panel has no accessibility violations", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Drop a firmware")).toBeVisible();
    await page.locator(".drop input").setInputFiles({ name: "bad.dfu", mimeType: "application/octet-stream", buffer: faulting() });
    await expect(page.getByRole("alert")).toContainText("Hang: 5 s", { timeout: 30_000 });
    await page.getByRole("tab", { name: "Diagnostics" }).click();
    const { violations } = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(violations.map((v) => v.id)).toEqual([]);
  });
});
