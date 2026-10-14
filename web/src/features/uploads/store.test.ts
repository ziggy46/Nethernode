import { beforeEach, describe, expect, it, vi } from "vitest";

import * as store from "./store";

import type { TusCallbacks, TusTarget } from "./tus";

const started: { file: File; target: TusTarget; callbacks: TusCallbacks; abort: ReturnType<typeof vi.fn> }[] =
  [];

vi.mock("./tus", () => ({
  startUpload: async (file: File, target: TusTarget, callbacks: TusCallbacks) => {
    const abort = vi.fn(async () => undefined);
    started.push({ file, target, callbacks, abort });
    return { abort };
  },
}));
vi.mock("@/lib/api", () => ({ csrfToken: async () => "token" }));

const target = { serverId: "s1", serverName: "SMP", dir: "world" };
const file = (name: string, size = 100) => new File([new Uint8Array(size)], name);

describe("upload queue", () => {
  beforeEach(() => {
    for (const u of store.getUploads()) store.cancel(u.key);
    store.clearEnded();
    started.length = 0;
  });

  it("sends three at a time and starts the next when one finishes", async () => {
    store.enqueue(
      target,
      ["a", "b", "c", "d"].map((n) => file(n)),
      { conflict: "rename", extract: false },
    );
    await vi.waitFor(() => expect(started.map((s) => s.file.name)).toEqual(["a", "b", "c"]));
    expect(started[0]?.target).toMatchObject({
      serverId: "s1",
      dir: "world",
      conflict: "rename",
      csrf: "token",
    });
    expect(store.getUploads().map((u) => u.status)).toEqual([
      "uploading",
      "uploading",
      "uploading",
      "queued",
    ]);

    started[0]?.callbacks.onSuccess("U1");
    await vi.waitFor(() => expect(started.map((s) => s.file.name)).toEqual(["a", "b", "c", "d"]));
    expect(store.getUploads()[0]?.status).toBe("finishing");

    store.uploadFinished("U1", {
      path: "world/a",
      message: "Uploaded world/a.",
      skipped: false,
      failed: false,
    });
    expect(store.getUploads()[0]).toMatchObject({ status: "done", path: "world/a" });
  });

  it("handles a result that arrives before the upload reports success", async () => {
    store.enqueue(target, [file("a")], { conflict: "skip", extract: false });
    await vi.waitFor(() => expect(started).toHaveLength(1));
    store.uploadFinished("U9", { path: "world/a", message: "Skipped a.", skipped: true, failed: false });
    started[0]?.callbacks.onSuccess("U9");
    expect(store.getUploads()[0]?.status).toBe("skipped");
  });

  it("pauses, resumes, and cancels", async () => {
    store.enqueue(target, [file("a")], { conflict: "rename", extract: false });
    await vi.waitFor(() => expect(started).toHaveLength(1));
    const key = store.getUploads()[0]?.key ?? "";
    store.pause(key);
    expect(started[0]?.abort).toHaveBeenCalledWith(false);
    expect(store.getUploads()[0]?.status).toBe("paused");
    expect(store.uploading()).toBe(false);

    store.resume(key);
    await vi.waitFor(() => expect(started).toHaveLength(2));
    store.cancel(key);
    expect(started[1]?.abort).toHaveBeenCalledWith(true);
    expect(store.getUploads()[0]?.status).toBe("cancelled");
    store.clearEnded();
    expect(store.getUploads()).toEqual([]);
  });

  it("shows errors and keeps progress", async () => {
    store.enqueue(target, [file("a", 1000)], { conflict: "rename", extract: false });
    await vi.waitFor(() => expect(started).toHaveLength(1));
    started[0]?.callbacks.onProgress(400, 1000);
    started[0]?.callbacks.onError("not enough free disk space");
    expect(store.getUploads()[0]).toMatchObject({
      status: "failed",
      sent: 400,
      message: "not enough free disk space",
    });
  });
});
