//! 内容与变更类型（v11 §1.2）。

use time::OffsetDateTime;

use crate::id::SlipId;

/// 内容版本号。
///
/// 进程内单调递增，**只在内容真正改变时递增**（成功修改、有效重载、恢复创建、
/// 接纳外部修复）。失败与 `Noop` 都不增。初始载入为 1，0 表示尚未载入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentRev(pub u64);

impl ContentRev {
    /// 下一个版本号：**饱和递增**。
    ///
    /// 为什么不是 `+ 1`：release 构建下 u64 溢出会**回绕到 0**，而 `content_rev` 是
    /// 冲突检测的依据——回绕等于把版本号倒退回"尚未载入"，后果比停住严重得多。
    /// 为什么不 panic：一个计数到顶不该让用户的待办应用崩掉。
    ///
    /// 到顶后停住是"已知的有损边界"：u64::MAX 次内容变更在本产品生命周期内不可达
    /// （按每次 1ms 计也要 5.8 亿年），这里的选择是**可预测地饱和**而不是失控地回绕。
    pub fn next(self) -> Self {
        ContentRev(self.0.saturating_add(1))
    }
}

/// 一张纸条的内容。**不含任何几何、展示或运行时状态。**
#[derive(Debug, Clone, PartialEq)]
pub struct Slip {
    pub id: SlipId,
    pub text: String,
    pub done: bool,
    pub created_at: OffsetDateTime,
    pub done_at: Option<OffsetDateTime>,
    pub archived_at: Option<OffsetDateTime>,
    /// 定稿标记：正文不可再改。
    ///
    /// 只锁**正文**，不锁完成与归档——锁住的待办仍然必须能被勾掉，
    /// 否则这个标记会阻止任务完成，与它的用途正好相反。
    pub locked: bool,
}

/// 拆条产出的草稿：正文 + 本次捕获的时间。
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub text: String,
    pub occurred_at: OffsetDateTime,
}

/// 对已有纸条的局部修改。
#[derive(Debug, Clone, PartialEq)]
pub enum Patch {
    SetText(String),
    SetDone {
        done: bool,
        at: OffsetDateTime,
    },
    Archive {
        at: OffsetDateTime,
    },
    /// 取消归档（恢复到默认列表，保留 `done` 与 `done_at`）。
    ///
    /// 为什么需要它：归档后纸条立刻从默认视图隐藏，用户没有开关可点回去，
    /// 撤销栈需要一个能只清 `archived_at` 而不擦除完成状态的逆操作。
    Unarchive,
    /// 切换定稿标记。锁定与解锁都必须可做，否则锁就是单向门。
    SetLocked(bool),
}

/// 一次写入批次中的单个变更。
///
/// 一批 `Mutation` 由 store 作**全有或全无**处理，且最多触发一次 rename。
#[derive(Debug, Clone, PartialEq)]
pub enum Mutation {
    Append(Slip),
    Patch { id: SlipId, patch: Patch },
}

/// 转换是否真的改变了内容。
///
/// `Noop` 用于避免"重复点完成"产生多余写盘（v11 §1.7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Changed,
    Noop,
}
