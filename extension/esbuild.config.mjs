// 默认同时构建 Chromium / Firefox；--browser=... 只构建指定平台。
import { build, context } from "esbuild";
import { cpSync, mkdirSync, readFileSync, rmSync, watch, writeFileSync } from "node:fs";
import { manifestForBrowser } from "./scripts/manifest.mjs";

const watching = process.argv.includes("--watch");
const selected = process.argv.find((arg) => arg.startsWith("--browser="))?.split("=")[1];
if (selected && !["chromium", "firefox"].includes(selected)) throw new Error("--browser 必须为 chromium 或 firefox");
const targets = selected ? [selected] : watching ? ["chromium"] : ["chromium", "firefox"];

for (const target of targets) {
  const outdir = target === "chromium" ? "dist" : "dist-firefox";
  const options = {
    entryPoints: ["src/background.ts", "src/popup.ts", "src/options.ts", "src/links.ts", "src/welcome.ts"],
    outdir,
    bundle: true,
    format: "iife",
    target: target === "chromium" ? "chrome120" : "firefox140",
    define: { __SWOOP_FIREFOX__: String(target === "firefox") },
    sourcemap: watching ? "inline" : false,
    logLevel: "info",
  };
  function copyPublic() {
    cpSync("public", outdir, { recursive: true });
    const manifest = manifestForBrowser(JSON.parse(readFileSync("public/manifest.json", "utf8")), target);
    writeFileSync(`${outdir}/manifest.json`, JSON.stringify(manifest, null, 2) + "\n");
  }
  rmSync(outdir, { recursive: true, force: true });
  mkdirSync(outdir);
  copyPublic();
  if (watching) {
    const ctx = await context(options);
    await ctx.watch();
    watch("public", { recursive: true }, copyPublic);
  } else { await build(options); }
}
