// 工具栏徽标：已连接不显示；断开显示灰点；上次交接失败且没看过显示红色 !

import type { RuntimeState } from "./settings";

export interface Badge {
  text: string;
  color: string;
  title: string;
}

export function badgeFor(state: RuntimeState, enabled: boolean): Badge {
  if (state.unseenFailure) {
    return { text: "!", color: "#e5484d", title: `Swoop：${state.lastResult?.error ?? "上次交接失败"}` };
  }
  if (!enabled) return { text: "", color: "#8b8d98", title: "Swoop：已关闭接管" };
  if (!state.connected) return { text: "•", color: "#8b8d98", title: "Swoop：未连接（请先启动 Swoop）" };
  return { text: "", color: "#4a6cf2", title: "Swoop：已连接" };
}
