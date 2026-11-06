// SPDX-License-Identifier: AGPL-3.0-or-later
class ResourceMonitor {
  #pending = 0;
  #version = 0;
  #waiter;
  #idleTimer;
  #deadlineTimer;
  #error;

  constructor(budgetMs = 10000) {
    this.deadline = new Promise((_, reject) => {
      this.#deadlineTimer = setTimeout(() => {
        const error = new Error(`rendering exceeded ${budgetMs} ms`);
        this.#stop(error);
        reject(error);
      }, budgetMs);
    });
    this.deadline.catch(() => {});
  }

  start() {
    this.#pending++;
    this.#version++;
    clearTimeout(this.#idleTimer);
    let finished = false;
    return () => {
      if (finished) return;
      finished = true;
      this.#pending--;
      this.#version++;
      this.#schedule();
    };
  }

  wait() {
    if (this.#error) return Promise.reject(this.#error);
    if (this.#waiter) throw new Error('resource idle wait already active');
    return new Promise((resolve, reject) => {
      this.#waiter = { resolve, reject };
      this.#schedule();
    });
  }

  unchanged(version) {
    return !this.#error && this.#pending === 0 && this.#version === version;
  }

  close() {
    clearTimeout(this.#deadlineTimer);
    this.#stop(new Error('renderer stopped'));
  }

  #schedule() {
    clearTimeout(this.#idleTimer);
    if (!this.#waiter || this.#pending || this.#error) return;
    this.#idleTimer = setTimeout(() => {
      const { resolve } = this.#waiter;
      this.#waiter = undefined;
      resolve(this.#version);
    }, 500);
  }

  #stop(error) {
    clearTimeout(this.#idleTimer);
    this.#error = error;
    this.#waiter?.reject(error);
    this.#waiter = undefined;
  }
}

module.exports = { ResourceMonitor };
