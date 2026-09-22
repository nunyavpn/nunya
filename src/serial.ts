/**
 * Work that must happen one at a time, in the order it was asked for: connecting and
 * disconnecting (`main.ts`).
 *
 * Starting takes seconds — the core, then the system proxy — and so does stopping, and each half
 * has an inverse that must not run in the middle of it. A Disconnect that ran while Connect was
 * still setting the system proxy once put the user's settings back and then had ours land on top,
 * pointing every browser at a port nothing listened on. Disabling the buttons is not enough on
 * its own: a server chosen from the map, a switch flipped in the popover, the tray's menu and a
 * block list arriving can all ask for a reconnect while something is under way. So all of it goes
 * through one queue.
 *
 * A job that fails does not stop the queue; the next one runs anyway. A job queued by `once`
 * under a key that is already waiting is the one already waiting: five switches flipped in a row
 * are one reconnect, which applies the last of them.
 *
 * Nothing here imports anything, so it runs under `node --test`.
 */

export class Serial {
  private tail: Promise<void> = Promise.resolve();
  /** Jobs queued by `once` that have not started yet, by key. */
  private waiting = new Map<string, Promise<void>>();

  /** Runs `work` after everything queued before it. */
  run(work: () => Promise<void>): Promise<void> {
    const job = this.tail.then(work);
    // The queue goes on past a failure; the caller of `run` still sees it.
    this.tail = job.catch(() => {});
    return job;
  }

  /** Like `run`, unless a job under the same key is already waiting to start: then that one. */
  once(key: string, work: () => Promise<void>): Promise<void> {
    const queued = this.waiting.get(key);
    if (queued) return queued;
    const job = this.run(() => {
      this.waiting.delete(key);
      return work();
    });
    this.waiting.set(key, job);
    return job;
  }
}
