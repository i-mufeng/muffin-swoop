/** Firefox 原生 Promise API 与 Chromium MV3 Promise API 共用。 */
export const browser = (globalThis as typeof globalThis & { browser?: typeof chrome }).browser ?? globalThis.chrome;
declare const __SWOOP_FIREFOX__: boolean;
export const isFirefox = typeof __SWOOP_FIREFOX__ !== "undefined" && __SWOOP_FIREFOX__;
