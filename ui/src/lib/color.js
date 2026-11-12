/**
 * 颜色对比度计算（R117）。
 *
 * 为什么值得一个模块：`--paper-edge` 是发丝边框，它在浅色模式下对比度很低——
 * 看起来"很雅致"，但如果跌到 WCAG 的下限以下，用户就分不清纸条边界在哪。
 * 这种事肉眼看不出来，只能算。
 *
 * 用的是 WCAG 2.x 的相对亮度公式，不是"看着差不多"。
 */

/** 把 `#rrggbb` 解析成 [r, g, b]（0–255）。非法输入返回 null。 */
export function parseHex(hex) {
  if (typeof hex !== "string") {
    return null;
  }
  const text = hex.trim();
  if (!/^#[0-9a-fA-F]{6}$/.test(text)) {
    return null;
  }
  return [
    parseInt(text.slice(1, 3), 16),
    parseInt(text.slice(3, 5), 16),
    parseInt(text.slice(5, 7), 16),
  ];
}

/**
 * 单个颜色通道的相对亮度（WCAG 定义：先线性化再按 0.2126/0.7152/0.0722 加权）。
 */
function channelLuminance(value) {
  const srgb = value / 255;
  return srgb <= 0.04045 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
}

/** 颜色的相对亮度（0 = 纯黑，1 = 纯白）。 */
export function relativeLuminance(hex) {
  const rgb = parseHex(hex);
  if (!rgb) {
    return null;
  }
  const [r, g, b] = rgb.map(channelLuminance);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/**
 * 两个颜色的对比度（1–21）。
 *
 * 参考阈值（WCAG 2.1）：
 * - 正文文字：≥ 4.5（大字号 ≥ 3）
 * - **非文字 UI 组件与图形边界：≥ 3**（1.4.11）——发丝边框属于这一类
 */
export function contrastRatio(a, b) {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  if (la === null || lb === null) {
    return null;
  }
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}
