import { afterEach, describe, expect, it, vi } from "vitest";
import { sendTask } from "./api";

afterEach(() => vi.unstubAllGlobals());
describe("桥接响应", () => {
  it("HTTP 200 非协议数据不能冒充接管成功", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("<html>another server</html>", { status: 200 })));
    expect(await sendTask(51230, { url: "https://x/a", headers: [] })).toMatchObject({ ok: false });
  });
  it("逐项保留服务器失败信息与成功文件名", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(Response.json({ error: "登录过期" }, { status: 422 }))
      .mockResolvedValueOnce(Response.json({ id: 1, file_name: "report.zip", size: 42 })));
    expect(await sendTask(51230, { url: "https://x/a", headers: [] })).toEqual({ ok: false, error: "登录过期" });
    expect(await sendTask(51230, { url: "https://x/b", headers: [] })).toEqual({ ok: true, fileName: "report.zip", size: 42 });
  });
});
