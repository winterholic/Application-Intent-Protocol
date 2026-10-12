import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";
import { test } from "node:test";
import { createGoogleStorageAdapter } from "../providers/google-storage.mjs";

function fakeStorage() {
  const objects = new Map();
  const streamOptions = [];
  const selected = [];
  return {
    objects,
    streamOptions,
    selected,
    bucket(name) {
      assert.equal(name, "bucket-a");
      return {
        file(objectName, options = {}) {
          selected.push({ objectName, options });
          const record = objects.get(objectName);
          return {
            createWriteStream(writeOptions) {
              streamOptions.push(writeOptions);
              const chunks = [];
              return new Writable({
                write(chunk, _encoding, done) {
                  chunks.push(Buffer.from(chunk));
                  done();
                },
                final(done) {
                  const data = Buffer.concat(chunks);
                  objects.set(objectName, { data, contentType: writeOptions.metadata.contentType, generation: "7", etag: "etag-7" });
                  done();
                },
              });
            },
            createReadStream() {
              if (!record) return Readable.from((async function* () { throw Object.assign(new Error("missing"), { code: 404 }); })());
              return Readable.from([record.data]);
            },
            async getMetadata() {
              const current = objects.get(objectName);
              if (!current) throw Object.assign(new Error("missing"), { code: 404 });
              return [{ size: String(current.data.length), contentType: current.contentType, generation: current.generation, etag: current.etag }];
            },
            async delete(options) {
              assert.equal(options.ignoreNotFound, true);
              if (!objects.has(objectName)) return;
              objects.delete(objectName);
            },
          };
        },
      };
    },
  };
}

const uuid = "7de0de57-f817-43f6-9de0-715e92310acd";

test("streams bounded objects through official SDK options and generation-safe cleanup", async () => {
  const storage = fakeStorage();
  const provider = createGoogleStorageAdapter({ storage, bucket: "bucket-a", maxBytes: 1024 });
  const owner = provider.forOwner("tenant-a");
  const payload = Buffer.from("streamed bytes");
  const result = await owner.upload(uuid, Readable.from([payload.subarray(0, 4), payload.subarray(4)]), {
    size: payload.length,
    contentType: "application/octet-stream",
  });
  assert.equal(result.size, payload.length);
  assert.equal(storage.streamOptions[0].resumable, true);
  assert.equal(storage.streamOptions[0].chunkSize, 8 * 1024 * 1024);
  assert.equal(storage.streamOptions[0].preconditionOpts.ifGenerationMatch, 0);
  const objectName = storage.selected[0].objectName;
  assert.match(objectName, /^[a-f0-9]{64}\//);

  const output = [];
  await owner.download(uuid, new Writable({ write(chunk, _encoding, done) { output.push(Buffer.from(chunk)); done(); } }));
  assert.deepEqual(Buffer.concat(output), payload);
  assert.equal((await owner.head(uuid)).generation, "7");
  assert.equal(await owner.delete(uuid), true);
  assert.equal(await owner.delete(uuid), false);
  assert.ok(storage.selected.some(({ options }) => options.generation === "7"), "reads and deletes must target an inspected generation");
});

test("rejects invalid IDs and streams whose byte count violates declared size", async () => {
  const storage = fakeStorage();
  const provider = createGoogleStorageAdapter({ storage, bucket: "bucket-a", maxBytes: 1024 });
  await assert.rejects(provider.upload("../escape", Readable.from([Buffer.from("x")]), { size: 1, contentType: "text/plain" }), /object id/i);
  await assert.rejects(provider.upload(uuid, Readable.from([Buffer.from("too long")]), { size: 2, contentType: "text/plain" }), /size/i);
  await assert.rejects(provider.upload(uuid, Readable.from([Buffer.from("x")]), { size: 1025, contentType: "text/plain" }), /limit/i);
  assert.equal(storage.objects.size, 0);
});
