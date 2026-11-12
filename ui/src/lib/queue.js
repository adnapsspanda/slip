/**
 * 变更命令的串行队列。
 *
 * 为什么需要：勾选、归档、编辑这些命令都带着"基于当前快照"的意图。用户在几十毫秒内
 * 连点两下，第二次读到的仍是旧快照，于是两次都发「设为已完成」——用户本意是切回来，
 * 结果只变一次。这是典型的丢失更新。
 *
 * 这个队列做两件事：
 * 1. **串行**：同一时刻只有一个变更在飞，后来的排队；
 * 2. **失败不阻塞**：某个任务抛错不会让整条链卡死（后续任务照常执行）。
 *
 * 队列本身与业务无关，可以单独验证（见 `tools/check-ui-logic.mjs`）。
 */

/** @returns {{run: <T>(task: () => Promise<T>) => Promise<T>, size: () => number}} */
export function createSerialQueue() {
  let tail = Promise.resolve();
  let pending = 0;

  return {
    /**
     * 排队执行一个任务。
     * @template T
     * @param {() => Promise<T>} task
     * @returns {Promise<T>}
     */
    run(task) {
      pending += 1;
      const result = tail.then(task, task);
      // 用 `.then(noop, noop)` 而不是 `.catch`：无论成功失败都把链推进下去，
      // 否则一次失败会让后面所有任务永远等不到轮次。
      tail = result.then(
        () => {
          pending -= 1;
        },
        () => {
          pending -= 1;
        },
      );
      return result;
    },
    /** 还有几个在排队（含正在执行的）。 */
    size() {
      return pending;
    },
  };
}

/**
 * 一条纸条下一次勾选应该变成什么状态。
 *
 * 关键点是：有"未落盘的意图"时**以意图为准**，而不是以快照为准。
 * 连点两下 → 第一下记录意图 `true`，第二下从 `true` 反转得到 `false`，
 * 于是最终状态正确回到未完成。
 *
 * @param {boolean} snapshotDone 快照里的状态（可能还没跟上用户的点击）
 * @param {boolean|undefined} intent 尚未落盘的意图
 * @returns {boolean}
 */
export function nextToggleTarget(snapshotDone, intent) {
  if (typeof intent === "boolean") {
    return !intent;
  }
  return !snapshotDone;
}

/**
 * 一次勾选请求结束后，那条未落盘的意图还要不要丢掉。
 *
 * 意图的职责是让「连点」以意图为准反转；它一旦比快照活得更久，就会反过来骗人。
 * 但删除时机不能只看"这一条请求的结果"：同一 id 可能还有排队中的点击。
 * `remaining` 是该 id 在**这次请求之后**还剩几条在途/排队；只要还有，就必须保留
 * 意图，否则第三次点击会从旧快照反转，终态与点击次数不符（实测过这个形状）。
 *
 * - 还有排队请求 → 保留；
 * - 没有排队了，且后端没接受 → 丢弃（下一次点击不能基于没发生过的状态）；
 * - 没有排队了，且快照已等于目标 → 丢弃（意图已落盘）；
 * - 没有排队了，但快照还没追上（刷新失败）→ 保留：意图等于目标，仍是下一次反转的正确起点。
 *
 * @param {number} remaining 该 id 在这次请求之后仍在途/排队的数量
 * @param {boolean} accepted 后端是否接受（`ack.result === "applied"`）
 * @param {boolean|undefined} storedDone 最新快照里这一条的完成态
 * @param {boolean} targetDone 这次请求的目标状态
 * @returns {boolean} true = 丢弃意图
 */
export function shouldDropPendingIntent(
  remaining,
  accepted,
  storedDone,
  targetDone,
) {
  if (remaining > 0) return false;
  return !accepted || storedDone === targetDone;
}
