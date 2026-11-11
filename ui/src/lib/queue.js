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
