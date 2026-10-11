import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { crc32, inflateRawSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import { zipDirectory } from "./zip.mjs";

/** 按中央目录读出全部条目，校验本地头与中央目录一致。 */
function readZip(zip) {
  const end = zip.length - 22;
  expect(zip.readUInt32LE(end)).toBe(0x06054b50);
  const count = zip.readUInt16LE(end + 10);
  let cursor = zip.readUInt32LE(end + 16);
  const entries = new Map();
  for (let i = 0; i < count; i++) {
    expect(zip.readUInt32LE(cursor)).toBe(0x02014b50);
    const crc = zip.readUInt32LE(cursor + 16);
    const size = zip.readUInt32LE(cursor + 20);
    const nameLength = zip.readUInt16LE(cursor + 28);
    const local = zip.readUInt32LE(cursor + 42);
    const name = zip.subarray(cursor + 46, cursor + 46 + nameLength).toString("utf8");
    expect(zip.readUInt32LE(local)).toBe(0x04034b50);
    expect(zip.subarray(local + 30, local + 30 + nameLength).toString("utf8")).toBe(name);
    const start = local + 30 + nameLength;
    const data = inflateRawSync(zip.subarray(start, start + size));
    expect(crc32(data)).toBe(crc);
    entries.set(name, data.toString("utf8"));
    cursor += 46 + nameLength;
  }
  return entries;
}

describe("zipDirectory", () => {
  const root = mkdtempSync(join(tmpdir(), "swoop-zip-"));
  mkdirSync(join(root, "_locales/zh_CN"), { recursive: true });
  writeFileSync(join(root, "manifest.json"), '{"name":"Swoop"}');
  writeFileSync(join(root, "_locales/zh_CN/messages.json"), '{"中文":"下载"}');
  writeFileSync(join(root, "empty.txt"), "");
  writeFileSync(join(root, ".DS_Store"), "junk");

  it("条目位于包根、正斜杠、排序且内容可还原", () => {
    const entries = readZip(zipDirectory(root));
    expect([...entries.keys()]).toEqual(["_locales/zh_CN/messages.json", "empty.txt", "manifest.json"]);
    expect(entries.get("manifest.json")).toBe('{"name":"Swoop"}');
    expect(entries.get("_locales/zh_CN/messages.json")).toBe('{"中文":"下载"}');
    expect(entries.get("empty.txt")).toBe("");
  });

  it("重复打包逐字节一致", () => {
    expect(zipDirectory(root).equals(zipDirectory(root))).toBe(true);
  });
});
