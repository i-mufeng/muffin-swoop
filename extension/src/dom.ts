/** 按 id 取元素，取不到直接抛错（页面结构写死，取不到就是 bug） */
export function $<T extends HTMLElement = HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`#${id} 不存在`);
  return el as T;
}
