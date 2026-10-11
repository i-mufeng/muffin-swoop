// 正式 A 标志以 swoop.svg 为母版；衍生资源均从它生成。
// node assets/brand/swoop/build-assets.mjs（需要 rsvg-convert；macOS 同时生成 ICNS）
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';

const root = dirname(fileURLToPath(import.meta.url));
const master = readFileSync(join(root, 'swoop.svg'), 'utf8').trim();
if (!master.includes('rotate(180 120 120)') || (master.match(/<use /g) || []).length !== 2) {
  throw new Error('母版必须保留同一单元的两个引用与 180° 旋转。');
}
const mono = color => master.replace(/#176FFF|#7C3CF4/g, color);
const app = mono('#FFFFFF')
  .replace('</defs>', '</defs><rect x="4" y="4" width="232" height="232" rx="59" fill="#4A6CF2"/><g transform="translate(34 34) scale(.716667)">')
  .replace('</svg>', '</g></svg>');
for (const [name, svg] of Object.entries({
  'swoop-mono': mono('#171B25'), 'swoop-white': mono('#FFFFFF'),
  'swoop-blue': mono('#4A6CF2'), 'swoop-app': app,
})) writeFileSync(join(root, `${name}.svg`), svg + '\n');

mkdirSync(join(root, 'png'), { recursive: true });
function render(svg, size, output) {
  execFileSync('rsvg-convert', ['-w', String(size), '-h', String(size), '-o', output, join(root, svg)]);
}
render('swoop.svg', 256, join(root, 'png/swoop-256.png'));
render('swoop-mono.svg', 44, join(root, 'png/swoop-tray-44.png'));
const sizes = [16, 24, 32, 48, 64, 128, 256, 512, 1024];
for (const size of sizes) render('swoop-app.svg', size, join(root, `png/swoop-app-${size}.png`));

// ICO 直接封装 PNG 帧，不引入图像处理依赖。
const icoSizes = [16, 24, 32, 48, 64, 128, 256];
const frames = icoSizes.map(size => readFileSync(join(root, `png/swoop-app-${size}.png`)));
const header = Buffer.alloc(6 + 16 * frames.length);
header.writeUInt16LE(1, 2); header.writeUInt16LE(frames.length, 4);
let offset = header.length;
frames.forEach((frame, i) => {
  const p = 6 + i * 16, size = icoSizes[i];
  header[p] = header[p + 1] = size === 256 ? 0 : size;
  header.writeUInt16LE(1, p + 4); header.writeUInt16LE(32, p + 6);
  header.writeUInt32LE(frame.length, p + 8); header.writeUInt32LE(offset, p + 12);
  offset += frame.length;
});
writeFileSync(join(root, 'swoop.ico'), Buffer.concat([header, ...frames]));

if (process.platform === 'darwin') {
  const scratch = mkdtempSync(join(tmpdir(), 'swoop-brand-'));
  try {
    const iconset = join(scratch, 'Swoop.iconset'); mkdirSync(iconset);
    for (const size of [16, 32, 128, 256, 512]) {
      copyFileSync(join(root, `png/swoop-app-${size}.png`), join(iconset, `icon_${size}x${size}.png`));
      copyFileSync(join(root, `png/swoop-app-${size * 2}.png`), join(iconset, `icon_${size}x${size}@2x.png`));
    }
    execFileSync('iconutil', ['-c', 'icns', iconset, '-o', join(root, 'swoop.icns')]);
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}

const extension = resolve(root, '../../../extension/public/icons');
copyFileSync(join(root, 'swoop-app.svg'), join(extension, 'icon.svg'));
for (const size of [16, 32, 48, 128]) {
  copyFileSync(join(root, `png/swoop-app-${size}.png`), join(extension, `icon-${size}.png`));
}
console.log('A 折光：SVG、PNG、ICO、扩展图标已生成' + (process.platform === 'darwin' ? '，ICNS 已生成。' : '。'));
