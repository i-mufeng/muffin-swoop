// 展示用的小工具，与 crates/app/src/ui/format.rs 的口径一致。
import { translate, type Language } from "./i18n";

export function bytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return i === 0 ? `${v} B` : `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`;
}

export function ago(at: number, now = Date.now(), language: Language = "zh-CN"): string {
  const s = Math.max(0, Math.floor((now - at) / 1000));
  if (s < 60) return translate("刚刚", language);
  if (s < 3600) return translate("{count} 分钟前", language, { count: Math.floor(s / 60) });
  if (s < 86_400) return translate("{count} 小时前", language, { count: Math.floor(s / 3600) });
  return translate("{count} 天前", language, { count: Math.floor(s / 86_400) });
}
