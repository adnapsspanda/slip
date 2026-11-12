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
 * 一次勾选请求结束后，那条未落盘的意图还要不要留着。
 *
 * 为什么需要这个判定：意图的唯一职责是让「连点两下」以意图为准反转。它一旦
 * 比快照活得更久，就会反过来骗人——上一轮失败留下的 `true` 会让用户下一次
 * 点击的目标变成 `false`，症状是「勾选要按两下才生效」。
 *
 * 三种情况：
 * - 后端没接受：快照不会朝目标动，留着意图等于让下一次点击基于一个不存在的状态；
 * - 快照已经等于目标：意图已经落盘，再留着会让下一次点击反转错方向；
 * - 其它：后面还有排队中的点击，意图仍是它们唯一可信的起点。
 *
 * @param {boolean} accepted 后端是否接受（`ack.result === "applied"`）
 * @param {boolean|undefined} storedDone 最新快照里这一条的完成态
 * @param {boolean} targetDone 这次请求的目标状态
 * @returns {boolean} true = 保留意图
 */
export function keepPendingIntent(accepted, storedDone, targetDone) {
  if (!accepted) return false;
  if (storedDone === targetDone) return false;
  return true;
}
