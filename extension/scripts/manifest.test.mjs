import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { manifestForBrowser } from "./manifest.mjs";

const base = JSON.parse(readFileSync(new URL("../public/manifest.json", import.meta.url), "utf8"));
describe("浏览器构建清单", () => {
  it("Chromium 只启用 service worker，包含主动页面提取权限", () => {
    const manifest = manifestForBrowser(base, "chromium");
    expect(manifest.background).toEqual({ service_worker: "background.js" });
    expect(manifest.permissions).toEqual(expect.arrayContaining(["activeTab", "scripting", "cookies", "storage", "webRequest"]));
    expect(manifest.permissions).not.toContain("webRequestBlocking");
    expect(manifest.action.default_popup).toBe("popup.html");
  });
  it("Firefox 只启用 scripts，包含稳定 ID 和传输数据声明且不改原始清单", () => {
    const manifest = manifestForBrowser(base, "firefox");
    expect(manifest.background).toEqual({ scripts: ["background.js"] });
    expect(manifest.minimum_chrome_version).toBeUndefined();
    expect(manifest.browser_specific_settings.gecko).toMatchObject({
      id: "swoop@muffin.imufeng.cn", strict_min_version: "140.0",
      data_collection_permissions: { required: ["authenticationInfo", "browsingActivity"] },
    });
    expect(base.background.service_worker).toBe("background.js");
  });
});
