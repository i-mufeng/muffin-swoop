// 从正式 A 母版统一生成图标（需要 rsvg-convert：brew install librsvg）。
// 请编辑 assets/brand/swoop/swoop.svg，不要独立修改扩展副本。
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

execFileSync(process.execPath, [fileURLToPath(new URL("../../assets/brand/swoop/build-assets.mjs", import.meta.url))], { stdio: "inherit" });
