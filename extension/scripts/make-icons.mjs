// 从 public/icons/icon.svg 生成各尺寸 PNG（需要 rsvg-convert：brew install librsvg）。
// 生成结果已提交到仓库，只有改了 SVG 才需要重跑。
import { execFileSync } from "node:child_process";

for (const size of [16, 32, 48, 128]) {
  execFileSync("rsvg-convert", ["-w", String(size), "-h", String(size), "-o", `public/icons/icon-${size}.png`, "public/icons/icon.svg"]);
  console.log(`icon-${size}.png`);
}
