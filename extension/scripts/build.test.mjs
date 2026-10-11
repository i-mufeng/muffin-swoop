import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { beforeAll, describe, expect, it } from "vitest";

beforeAll(() => {
  execFileSync(process.execPath, ["esbuild.config.mjs"], { stdio: "pipe" });
}, 30_000);

describe("Chromium / Firefox 实际构建资源", () => {
  for (const directory of ["dist", "dist-firefox"]) {
    it(`${directory} 包含可解析的双语 manifest 与所有页面资源`, () => {
      const read = (path) => readFileSync(`${directory}/${path}`, "utf8");
      const manifest = JSON.parse(read("manifest.json"));
      expect(manifest.default_locale).toBe("zh_CN");
      expect(manifest.description).toBe("__MSG_extensionDescription__");
      for (const locale of ["zh_CN", "en"]) {
        const messages = JSON.parse(read(`_locales/${locale}/messages.json`));
        for (const [, key] of JSON.stringify(manifest).matchAll(/__MSG_(\w+)__/g)) {
          expect(messages[key].message).toBeTruthy();
        }
      }
      expect(JSON.parse(read("_locales/en/messages.json")).extensionDescription.message).toContain("Send browser downloads");
      for (const page of ["popup", "options", "links"]) {
        const html = read(`${page}.html`);
        expect(html).toContain("data-i18n=");
        for (const [, resource] of html.matchAll(/(?:src|href)="([^"#]+)"/g)) {
          expect(existsSync(`${directory}/${resource}`), resource).toBe(true);
        }
        const bundle = read(`${page}.js`);
        expect(bundle).toContain("en-US");
        expect(bundle).toContain("zh-CN");
        expect(bundle).toContain("Download page links");
      }
      expect(read("options.html")).toContain('<option value="en-US">English</option>');
      expect(read("background.js")).toContain("Download with Swoop");
      expect(manifest.background).toEqual(directory === "dist" ? { service_worker: "background.js" } : { scripts: ["background.js"] });
    });
  }
});
