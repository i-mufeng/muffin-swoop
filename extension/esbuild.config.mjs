// 打包 src/*.ts → dist/*.js，并把 public/ 原样拷到 dist/。`--watch` 时监听两者。
import { build, context } from "esbuild";
import { cpSync, mkdirSync, rmSync, watch } from "node:fs";

const watching = process.argv.includes("--watch");
const options = {
  entryPoints: ["src/background.ts", "src/popup.ts", "src/options.ts"],
  outdir: "dist",
  bundle: true,
  format: "iife",
  target: "chrome120",
  sourcemap: watching ? "inline" : false,
  logLevel: "info",
};

function copyPublic() {
  cpSync("public", "dist", { recursive: true });
}

rmSync("dist", { recursive: true, force: true });
mkdirSync("dist");
copyPublic();

if (watching) {
  const ctx = await context(options);
  await ctx.watch();
  watch("public", { recursive: true }, copyPublic);
} else {
  await build(options);
}
