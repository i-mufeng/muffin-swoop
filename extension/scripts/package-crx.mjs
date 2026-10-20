// 把已构建的 dist/ 打成签名 CRX3 拖拽安装包，并产出带顶层文件夹的解压安装 zip。
// 同时把 dist-firefox/ 打成 .xpi（Firefox 企业策略静默安装用）。
// 签名密钥固定为 dist.pem，因此扩展 ID 恒定（改密钥 = 换 ID，勿动）。
// 用法：node scripts/package-crx.mjs [--out=目录]，默认输出到仓库的 target/packages/。
import { createHash } from "node:crypto";
import { createPrivateKey, createPublicKey, sign, verify } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { zipDirectory } from "./zip.mjs";

const out = resolve(process.argv.find((arg) => arg.startsWith("--out="))?.slice(6) ?? "../target/packages");
const version = JSON.parse(readFileSync("package.json", "utf8")).version;
const manifestVersion = JSON.parse(readFileSync("dist/manifest.json", "utf8")).version;
if (manifestVersion !== version) {
  throw new Error(`dist/manifest.json 版本 ${manifestVersion} 与 package.json 版本 ${version} 不一致，先运行 npm run build`);
}

mkdirSync(out, { recursive: true });

/** CRX3 的 protobuf 编码：varint 长度 + bytes 字段。 */
const varint = (n) => {
  const bytes = [];
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n) b |= 0x80;
    bytes.push(b);
  } while (n);
  return Buffer.from(bytes);
};
const fieldBytes = (num, data) => Buffer.concat([varint((num << 3) | 2), varint(data.length), data]);
const u32le = (n) => {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n, 0);
  return b;
};

/** 用 dist.pem 把 zip 包签成 CRX3，返回 { crx, id } 并做完整自检。 */
export function buildCrx(zip, pem) {
  const key = createPrivateKey(pem);
  const pubDer = createPublicKey(key).export({ type: "spki", format: "der" });
  const crxId = createHash("sha256").update(pubDer).digest().subarray(0, 16);

  const signedData = fieldBytes(1, crxId); // SignedData { crx_id }
  // Chromium（crx3）签名输入：context + LE32(SignedData 长度) + SignedData + zip（无字段标签）
  const toSign = Buffer.concat([Buffer.from("CRX3 SignedData\0", "binary"), u32le(signedData.length), signedData, zip]);
  const signed = sign("RSA-SHA256", toSign, key);
  const header = Buffer.concat([
    fieldBytes(1, fieldBytes(1, Buffer.concat([fieldBytes(1, pubDer), fieldBytes(2, signed)]))), // sha256_with_rsa
    fieldBytes(2, signedData), // signed_header_data
  ]);
  const crx = Buffer.concat([Buffer.from("Cr24"), u32le(3), u32le(header.length), header, zip]);

  // 自检：按解析路径验证签名，失败直接中止打包
  if (!verify("RSA-SHA256", toSign, createPublicKey(key), signed)) {
    throw new Error("CRX3 自签名校验失败");
  }
  const id = [...crxId].map((b) => String.fromCharCode(97 + (b >> 4)) + String.fromCharCode(97 + (b & 0xf))).join("");
  return { crx, id };
}

const zip = zipDirectory("dist");
const { crx, id } = buildCrx(zip, readFileSync("dist.pem"));

// 随包信息文件：browser-bridge 与安装包据此拿到扩展 ID 和版本（稳定文件名，不含版本号）
writeFileSync(
  resolve(out, "swoop-extension-info.json"),
  JSON.stringify({ chromium_id: id, version }, null, 2) + "\n",
);

const name = `swoop-extension-chromium-${version}.crx`;
writeFileSync(resolve(out, name), crx);
writeFileSync(
  resolve(out, `${name}.sha256`),
  `${createHash("sha256").update(crx).digest("hex")}  ${name}\n`,
);
console.log(`${resolve(out, name)} (${crx.length} bytes, id ${id})`);

const unpacked = zipDirectory("dist", "swoop-extension");
const unpackedName = `swoop-extension-chromium-${version}-unpacked.zip`;
writeFileSync(resolve(out, unpackedName), unpacked);
console.log(`${resolve(out, unpackedName)} (${unpacked.length} bytes)`);

if (existsSync("dist-firefox/manifest.json")) {
  const xpi = zipDirectory("dist-firefox");
  const xpiName = `swoop-extension-firefox-${version}.xpi`;
  writeFileSync(resolve(out, xpiName), xpi);
  writeFileSync(resolve(out, `${xpiName}.sha256`), `${createHash("sha256").update(xpi).digest("hex")}  ${xpiName}\n`);
  console.log(`${resolve(out, xpiName)} (${xpi.length} bytes)`);
}
