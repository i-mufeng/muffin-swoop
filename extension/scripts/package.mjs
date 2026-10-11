// 把已构建的 dist/、dist-firefox/ 打成商店可上传的 zip，并写同名 .sha256。
// 用法：node scripts/package.mjs [--out=目录]，默认输出到仓库的 target/packages/。
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { zipDirectory } from "./zip.mjs";

const out = resolve(process.argv.find((arg) => arg.startsWith("--out="))?.slice(6) ?? "../target/packages");
const version = JSON.parse(readFileSync("package.json", "utf8")).version;

mkdirSync(out, { recursive: true });
for (const [browser, directory] of [["chromium", "dist"], ["firefox", "dist-firefox"]]) {
  if (!existsSync(join(directory, "manifest.json"))) throw new Error(`${directory}/ 未构建，先运行 npm run build`);
  const manifestVersion = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8")).version;
  if (manifestVersion !== version) {
    throw new Error(`${directory}/manifest.json 版本 ${manifestVersion} 与 package.json 版本 ${version} 不一致`);
  }
  const name = `swoop-extension-${browser}-${version}.zip`;
  const zip = zipDirectory(directory);
  writeFileSync(join(out, name), zip);
  writeFileSync(join(out, `${name}.sha256`), `${createHash("sha256").update(zip).digest("hex")}  ${name}\n`);
  console.log(`${join(out, name)} (${zip.length} bytes)`);
}
