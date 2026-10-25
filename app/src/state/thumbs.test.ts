// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { ThumbCache } from "./thumbs";

function harness(fail: Set<number> = new Set()) {
  const pending: Array<{ frame: number; resolve: () => void; reject: () => void }> = [];
  const fetched: number[] = [];
  const revoked: string[] = [];
  const changes: Array<[number, string | null]> = [];
  let n = 0;
  const cache = new ThumbCache({
    fetch: (frame) =>
      new Promise<ArrayBuffer>((resolve, reject) => {
        fetched.push(frame);
        pending.push({
          frame,
          resolve: () => (fail.has(frame) ? reject(new Error("no")) : resolve(new ArrayBuffer(1))),
          reject: () => reject(new Error("no")),
        });
      }),
    makeUrl: () => `blob:${(n += 1)}`,
    revoke: (url) => revoked.push(url),
    onChange: (frame, url) => changes.push([frame, url]),
  });
  const settle = async () => {
    while (pending.length > 0) {
      pending.shift()?.resolve();
      await new Promise((r) => setTimeout(r, 0));
    }
  };
  return { cache, pending, fetched, revoked, changes, settle };
}

describe("ThumbCache", () => {
  it("runs at most three fetches at once", async () => {
    const h = harness();
    for (let f = 0; f < 6; f += 1) h.cache.request(f, 0);
    expect(h.cache.running).toBe(3);
    expect(h.fetched).toEqual([0, 1, 2]);
    await h.settle();
    expect(h.fetched).toEqual([0, 1, 2, 3, 4, 5]);
    expect(h.changes).toHaveLength(6);
  });

  it("serves a cached frame without fetching again", async () => {
    const h = harness();
    h.cache.request(0, 0);
    h.cache.request(0, 0);
    await h.settle();
    h.cache.request(0, 0);
    expect(h.fetched).toEqual([0]);
  });

  it("refetches a stale frame and revokes the replaced URL", async () => {
    const h = harness();
    h.cache.request(0, 0);
    await h.settle();
    h.cache.request(0, 5);
    await h.settle();
    expect(h.fetched).toEqual([0, 0]);
    expect(h.changes).toEqual([
      [0, "blob:1"],
      [0, "blob:2"],
    ]);
    expect(h.revoked).toEqual(["blob:1"]);
  });

  it("fetches again when marked stale during a fetch", async () => {
    const h = harness();
    h.cache.request(0, 0);
    h.cache.request(0, 3);
    await h.settle();
    expect(h.fetched).toEqual([0, 0]);
  });

  it("does not retry a failure until the frame is stale again", async () => {
    const h = harness(new Set([1]));
    h.cache.request(1, 0);
    await h.settle();
    h.cache.request(1, 0);
    expect(h.fetched).toEqual([1]);
    expect(h.changes).toEqual([[1, null]]);
    h.cache.request(1, 2);
    expect(h.fetched).toEqual([1, 1]);
  });

  it("reset revokes every URL and drops results of fetches it overtook", async () => {
    const h = harness();
    h.cache.request(0, 0);
    await h.settle();
    h.cache.request(1, 0);
    h.cache.reset();
    expect(h.revoked).toEqual(["blob:1"]);
    await h.settle();
    expect(h.changes).toEqual([[0, "blob:1"]]);
    h.cache.request(0, 0);
    expect(h.fetched).toEqual([0, 1, 0]);
  });
});
