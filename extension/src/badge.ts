// 工具栏徽标：已连接不显示；断开显示灰点；上次交接失败且没看过显示红色 !

import type { RuntimeState } from "./settings";
import { translate, translateError, type Language } from "./i18n";

export interface Badge {
  text: string;
  color: string;
  title: string;
}

export function badgeFor(state: RuntimeState, enabled: boolean, language: Language = "zh-CN"): Badge {
  if (state.unseenFailure) {
    return { text: "!", color: "#e5484d", title: translate("Swoop：{error}", language,
      { error: translateError(state.lastResult?.error ?? "上次交接失败", language) }) };
  }
  if (!enabled) return { text: "", color: "#8b8d98", title: translate("Swoop：已关闭接管", language) };
  if (!state.connected) return { text: "•", color: "#8b8d98", title: translate("Swoop：未连接（请先启动 Swoop）", language) };
  return { text: "", color: "#4a6cf2", title: translate("Swoop：已连接", language) };
}
