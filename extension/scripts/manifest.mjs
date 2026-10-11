/** 每个平台只包含自己支持的 MV3 后台声明。 */
export function manifestForBrowser(base, target) {
  const manifest = structuredClone(base);
  if (target === "chromium") return manifest;
  if (target !== "firefox") throw new Error(`Unknown browser: ${target}`);
  delete manifest.minimum_chrome_version;
  manifest.background = { scripts: ["background.js"] };
  manifest.browser_specific_settings = {
    gecko: {
      id: "swoop@muffin.imufeng.cn",
      strict_min_version: "140.0",
      data_collection_permissions: { required: ["authenticationInfo", "browsingActivity"] },
    },
  };
  return manifest;
}
