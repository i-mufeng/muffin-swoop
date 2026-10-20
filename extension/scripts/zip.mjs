// 无依赖的确定性 zip：条目按路径排序、固定时间戳、正斜杠路径，跨平台产出逐字节一致。
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { crc32, deflateRawSync } from "node:zlib";

/** DOS 日期 1980-01-01，时间 00:00:00。 */
const DOS_DATE = (0 << 9) | (1 << 5) | 1;
const DOS_TIME = 0;
/** 通用标志位 11：文件名为 UTF-8。 */
const UTF8_FLAG = 0x0800;

/** 列出目录下全部文件（相对路径、正斜杠），跳过点文件（.DS_Store 等）。 */
export function listFiles(root, prefix = "") {
  const files = [];
  for (const entry of readdirSync(join(root, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) files.push(...listFiles(root, path));
    else if (entry.isFile()) files.push(path);
  }
  return files.sort();
}

/** 把目录内容打成 zip（目录本身不进包，manifest.json 位于包根）。
 *  prefix 给所有条目加顶层文件夹（如 "swoop-extension"），解压安装用。 */
export function zipDirectory(root, prefix = "") {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const path of listFiles(root)) {
    const data = readFileSync(join(root, path));
    const compressed = deflateRawSync(data, { level: 9 });
    const name = Buffer.from(prefix ? `${prefix}/${path}` : path, "utf8");
    const crc = crc32(data);

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(UTF8_FLAG, 6);
    local.writeUInt16LE(8, 8);
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(compressed.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    locals.push(local, name, compressed);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(UTF8_FLAG, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt16LE(DOS_TIME, 12);
    central.writeUInt16LE(DOS_DATE, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(compressed.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, name);

    offset += local.length + name.length + compressed.length;
    if (offset > 0xffffffff) throw new Error("扩展包超过 4 GiB，需要 zip64");
  }
  const entries = centrals.length / 2;
  if (entries > 0xffff) throw new Error("扩展包条目超过 65535，需要 zip64");
  const centralSize = centrals.reduce((sum, part) => sum + part.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries, 8);
  end.writeUInt16LE(entries, 10);
  end.writeUInt32LE(centralSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, ...centrals, end]);
}
