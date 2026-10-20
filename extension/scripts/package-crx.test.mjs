// package-crx.mjs 的端到端自检：用临时 RSA 密钥打一个 CRX3，
// 再按 Chromium 的解析路径完整走一遍（protobuf 解析 → 重建签名输入 → 验签）。
import { generateKeyPairSync, createHash, createPublicKey, verify } from "node:crypto";
import { describe, expect, it } from "vitest";
import { buildCrx } from "./package-crx.mjs";
import { zipDirectory } from "./zip.mjs";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function readVarint(buf, i) {
  let n = 0;
  let shift = 0;
  for (;;) {
    const b = buf[i++];
    n |= (b & 0x7f) << shift;
    shift += 7;
    if (!(b & 0x80)) return [n, i];
  }
}
/** 取 bytes 字段：返回 [值, 下一个偏移]。 */
function readField(buf, i) {
  const [key, j] = readVarint(buf, i);
  const [len, k] = readVarint(buf, j);
  return [key >> 3, buf.subarray(k, k + len), k + len];
}

describe("buildCrx", () => {
  it("产出可通过 Chromium 验签路径的 CRX3", () => {
    const dir = mkdtempSync(join(tmpdir(), "swoop-crx-test-"));
    mkdirSync(join(dir, "dist"));
    writeFileSync(join(dir, "dist", "manifest.json"), JSON.stringify({ manifest_version: 3, version: "0.0.1" }));
    const zip = zipDirectory(join(dir, "dist"));
    const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    const pem = privateKey.export({ type: "pkcs8", format: "pem" });
    const { crx, id } = buildCrx(zip, pem);
    rmSync(dir, { recursive: true, force: true });

    expect(crx.subarray(0, 4).toString("binary")).toBe("Cr24");
    expect(crx.readUInt32LE(4)).toBe(3);
    const headerLen = crx.readUInt32LE(8);
    const header = crx.subarray(12, 12 + headerLen);
    expect(crx.subarray(12 + headerLen)).toEqual(zip);

    // CrxFileHeader：field 1 = sha256_with_rsa，field 2 = signed_header_data
    const [f1, sigSetRaw, off] = readField(header, 0);
    expect(f1).toBe(1);
    const [f2, signedData] = readField(header, off);
    expect(f2).toBe(2);

    // SignatureSet -> Signature { public_key, signature }
    const [, sigRaw] = readField(sigSetRaw, 0);
    const [, pubRaw, off2] = readField(sigRaw, 0);
    const [, sig] = readField(sigRaw, off2);

    // SignedData { crx_id }
    const [, crxId] = readField(signedData, 0);
    const pubDer = createPublicKey({ key: pubRaw, format: "der", type: "spki" }).export({ type: "spki", format: "der" });
    expect([...createHash("sha256").update(pubDer).digest().subarray(0, 16)]).toEqual([...crxId]);
    expect(id).toHaveLength(32);

    // 重建签名输入并验签（与 crx3.cc 一致）
    const toSign = Buffer.concat([
      Buffer.from("CRX3 SignedData\0", "binary"),
      (() => {
        const b = Buffer.alloc(4);
        b.writeUInt32LE(signedData.length, 0);
        return b;
      })(),
      signedData,
      zip,
    ]);
    expect(verify("RSA-SHA256", toSign, createPublicKey({ key: pubDer, format: "der", type: "spki" }), sig)).toBe(true);
  });
});
