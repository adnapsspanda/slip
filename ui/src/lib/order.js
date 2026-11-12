/**
 * 未归档列表的**展示顺序**：置顶的稳定浮到最前，同组内保持原有顺序。
 *
 * 为什么抽成纯函数：排序是"稳定"这件事最容易做错——用 `sort((a,b) => b.pinned - a.pinned)`
 * 在 V8 上恰好稳定，但那依赖引擎实现；换成带时间戳的比较又会在同一毫秒内乱序。
 * 这里用**显式两趟分组**表达"稳定"，并把边界（空表、单条、字段缺失、输入不被改写）
 * 全部钉进 `tools/check-ui-logic.mjs`。
 *
 * 只读：返回新数组，绝不原地排序入参——快照里的数组是所有派生视图共用的，
 * 原地排一次序等于悄悄改了别人的世界。
 *
 * @param {Array<{pinned?: boolean}>} slips 已经是创建时间顺序的未归档列表
 * @returns {Array} 置顶组在前、其余在后；组内顺序不变
 */
export function orderSlips(slips) {
  if (!Array.isArray(slips)) return [];
  if (slips.length < 2) return slips.slice();
  const pinned = [];
  const rest = [];
  for (const slip of slips) {
    // 字段缺失/为假一律当"未置顶"：旧快照里没有 pinned 时不能凭空浮起来。
    // 用真值判定而不是 `=== true`：后端 serde 出来就是 bool，但测试桩/未来字段
    // 若给了别的真值，也应当按"置顶"理解，而不是悄悄掉回普通组。
    if (slip?.pinned) {
      pinned.push(slip);
    } else {
      rest.push(slip);
    }
  }
  // 没有置顶时连数组都不必重排，省一次拼接。
  return pinned.length === 0 ? slips.slice() : [...pinned, ...rest];
}
