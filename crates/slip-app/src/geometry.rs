//! 召唤窗口的纯几何：算出窗口左上角该落在哪里。
//!
//! 这些计算原本住在 `src-tauri/src/hotkey.rs`，但那个 crate 依赖 tauri，**测试只能在
//! Windows 上跑**——而本机 Windows 的测试二进制时不时被 Smart App Control 拒绝执行，
//! 于是最容易出错的边界逻辑（负坐标副屏、翻边、夹紧）长期只能靠端到端脚本"碰运气"，
//! 而那条脚本在单屏机器上还会 SKIP。
//!
//! 挪到这里（`slip-app` 不依赖 tauri）之后，这些断言在任何平台都能跑，包括 CI 的 Linux。

/// 一个矩形区域（物理像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// 算出窗口左上角应该落在哪里。
///
/// 顺序不能改：先尝试右下（`cursor + gap`），放不下再翻到左上，最后夹紧进工作区。
/// 夹紧用 `.max()` 兜住"窗口比工作区还大"的退化情形——`clamp` 在 `min > max` 时会 panic。
pub fn place(cursor: (i32, i32), area: Area, size: (i32, i32), gap: i32) -> (i32, i32) {
    let (cursor_x, cursor_y) = cursor;
    let (width, height) = size;

    let mut x = cursor_x + gap;
    if x + width > area.right {
        x = cursor_x - gap - width;
    }
    let mut y = cursor_y + gap;
    if y + height > area.bottom {
        y = cursor_y - gap - height;
    }

    let max_x = (area.right - width).max(area.left);
    let max_y = (area.bottom - height).max(area.top);
    (x.clamp(area.left, max_x), y.clamp(area.top, max_y))
}

#[cfg(test)]
mod tests {
    use super::{Area, place};

    const WIN: (i32, i32) = (420, 560);
    const GAP: i32 = 16;

    /// 主屏：1920x1080，工作区被任务栏占了底部。
    fn primary() -> Area {
        Area {
            left: 0,
            top: 0,
            right: 1707,
            bottom: 1019,
        }
    }

    #[test]
    fn right_below_when_it_fits() {
        // 左上角放得下：直接落在光标右下方，不留意外偏移。
        assert_eq!(place((200, 200), primary(), WIN, GAP), (216, 216));
    }

    #[test]
    fn flips_left_when_it_would_overflow() {
        // 1400 + 16 + 420 = 1836 > 1707 → 翻到光标左侧。
        let (x, _) = place((1400, 100), primary(), WIN, GAP);
        assert_eq!(x, 1400 - GAP - 420);
    }

    #[test]
    fn flips_up_when_it_would_overflow() {
        // 900 + 16 + 560 = 1476 > 1019 → 翻到光标上方。
        let (_, y) = place((100, 900), primary(), WIN, GAP);
        assert_eq!(y, 900 - GAP - 560);
    }

    #[test]
    fn exactly_at_the_edge_does_not_flip() {
        // 判据是严格大于：正好贴边仍算放得下。
        let cursor_x = 1707 - 420 - GAP;
        let (x, _) = place((cursor_x, 100), primary(), WIN, GAP);
        assert_eq!(x, cursor_x + GAP);

        // 再多 1 像素就必须翻。
        let (x, _) = place((cursor_x + 1, 100), primary(), WIN, GAP);
        assert_eq!(x, cursor_x + 1 - GAP - 420);
    }

    #[test]
    fn handles_monitor_with_negative_origin() {
        // 副屏整体落在负坐标区（开发机 DISPLAY5 就是这种布局）。
        let secondary = Area {
            left: -454,
            top: -1440,
            right: 2106,
            bottom: -48,
        };
        let (x, y) = place((-300, -1000), secondary, WIN, GAP);
        assert_eq!((x, y), (-284, -984));
        assert!(x >= secondary.left && x + WIN.0 <= secondary.right);
        assert!(y >= secondary.top && y + WIN.1 <= secondary.bottom);
    }

    #[test]
    fn clamps_when_window_is_larger_than_work_area() {
        // 退化情形：窗口比工作区还大。必须保住左上角可见，且**不能 panic**
        // （`clamp` 在 min > max 时会炸，靠 `.max()` 兜住）。
        let tiny = Area {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        };
        assert_eq!(place((250, 150), tiny, WIN, GAP), (0, 0));
    }

    #[test]
    fn stays_inside_work_area_for_every_edge_cursor() {
        // 扫一遍边界：无论光标落在工作区的哪个角落，结果都必须完整落在工作区内。
        let cases = [
            (-1000, -1000),
            (0, 0),
            (1707, 0),
            (0, 1019),
            (1707, 1019),
            (850, 500),
        ];
        for (cx, cy) in cases {
            let (x, y) = place((cx, cy), primary(), WIN, GAP);
            assert!(x >= 0, "cursor=({cx},{cy}) x={x} 越出左边");
            assert!(x + WIN.0 <= 1707, "cursor=({cx},{cy}) x={x} 越出右边");
            assert!(y >= 0, "cursor=({cx},{cy}) y={y} 越出上边");
            assert!(y + WIN.1 <= 1019, "cursor=({cx},{cy}) y={y} 越出下边");
        }
    }

    /// 断言窗口完整落在给定工作区内。
    ///
    /// **必须把光标摆到边角**才验得动夹紧/翻边分支——只在中间取点，删掉 `clamp`
    /// 这些测试也照样绿（实测过：第一次写的拓扑测试在变异下全过，等于没验）。
    fn assert_inside(area: Area, cursor: (i32, i32), label: &str) {
        let (x, y) = place(cursor, area, WIN, GAP);
        assert!(x >= area.left, "{label} cursor={cursor:?} x={x} 越出左边");
        assert!(
            x + WIN.0 <= area.right,
            "{label} cursor={cursor:?} x={x} 越出右边"
        );
        assert!(y >= area.top, "{label} cursor={cursor:?} y={y} 越出上边");
        assert!(
            y + WIN.1 <= area.bottom,
            "{label} cursor={cursor:?} y={y} 越出下边"
        );
    }

    /// 对一块工作区取五个代表性光标位置：四角 + 正中。
    fn assert_every_corner(area: Area, label: &str) {
        let mid_x = (area.left + area.right) / 2;
        let mid_y = (area.top + area.bottom) / 2;
        let cursors = [
            // 四角与正中：验翻边与常规路径
            (area.left, area.top),
            (area.right - 1, area.top),
            (area.left, area.bottom - 1),
            (area.right - 1, area.bottom - 1),
            (mid_x, mid_y),
            // 区域**外**的四点：光标可能落在屏幕边缘、任务栏或另一块屏上，
            // 这时翻边之后仍可能越界，**只有夹紧能兜住**。
            // 实测教训：不测这四个点，把 `clamp` 整段删掉这些测试也照样绿——等于没验。
            (area.left - 200, mid_y),
            (area.right + 200, mid_y),
            (mid_x, area.top - 200),
            (mid_x, area.bottom + 200),
        ];
        for cursor in cursors {
            assert_inside(area, cursor, label);
        }
    }

    // ---------- 以下四条对应 R62 要求的四种多屏拓扑 ----------

    /// 拓扑一：主屏在左（原点 0,0），副屏在**右边**延伸。
    #[test]
    fn topology_primary_left_secondary_right() {
        let area = Area {
            left: 1920,
            top: 0,
            right: 4480,
            bottom: 1440,
        };
        assert_every_corner(area, "副屏在右");
    }

    /// 拓扑二：主屏在右（原点 0,0），副屏在**左边**（整块负坐标）。
    #[test]
    fn topology_primary_right_secondary_left() {
        let area = Area {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_every_corner(area, "副屏在左");
        // 这条拓扑的关键：结果必须是负坐标，而不是被"夹"回 0（那会把窗口丢到另一块屏上）。
        let (x, _) = place((-100, 100), area, WIN, GAP);
        assert!(x < 0, "副屏在左时 x 应当为负，实际 {x}");
    }

    /// 拓扑三：副屏在**上方**（负 Y）。任务栏在主屏底部，副屏工作区整体为负 Y。
    #[test]
    fn topology_secondary_above_with_negative_y() {
        let area = Area {
            left: 0,
            top: -1440,
            right: 1920,
            bottom: -40,
        };
        assert_every_corner(area, "副屏在上");
        let (_, y) = place((600, -200), area, WIN, GAP);
        assert!(y < 0, "上方副屏 y 应当为负，实际 {y}");
    }

    /// 拓扑四：相邻屏分辨率不同（如 4K 副屏），工作区尺寸按物理像素给。
    /// 只验几何：物理像素坐标系下，窗口必须完整落在**自己那块**屏的工作区里。
    #[test]
    fn topology_secondary_with_different_scale() {
        let area = Area {
            left: 1920,
            top: 0,
            right: 5760,
            bottom: 2100,
        };
        assert_every_corner(area, "高 DPI 副屏");
    }
}
