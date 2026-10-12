import { createHash } from "node:crypto";
import { Transform } from "node:stream";
import { pipeline } from "node:stream/promises";

const DEFAULT_MAX_BYTES = 5 * 1024 * 1024 * 1024;
const MAX_GCS_OBJECT_BYTES = 5 * 1024 ** 4;
const CHUNK_BYTES = 8 * 1024 * 1024;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

/**
 * Builds a server-side adapter around an injected Google Cloud Storage client.
 * The injected client is the official @google-cloud/storage Storage instance.
 */
export function createGoogleStorageAdapter({ storage, bucket, maxBytes = DEFAULT_MAX_BYTES, uploadTimeoutMs = 60_000 }) {
  if (!storage || typeof storage.bucket !== "function") throw new TypeError("storage client is required");
  if (typeof bucket !== "string" || !/^[a-z0-9][a-z0-9._-]{1,61}[a-z0-9]$/.test(bucket)) throw new TypeError("invalid bucket");
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0 || maxBytes > MAX_GCS_OBJECT_BYTES) throw new TypeError("invalid object size limit");
  if (!Number.isSafeInteger(uploadTimeoutMs) || uploadTimeoutMs < 1 || uploadTimeoutMs > 10 * 60_000) throw new TypeError("invalid upload timeout");

  return scopedAdapter({ storage, bucket, maxBytes, uploadTimeoutMs, prefix: "" });
}

/** Loads the official SDK lazily; its Storage() constructor uses ADC. */
export async function createGoogleStorageProvider(options) {
  if (options.storage) return createGoogleStorageAdapter(options);
  let sdk;
  try {
    sdk = await import("@google-cloud/storage");
  } catch (error) {
    if (error?.code === "ERR_MODULE_NOT_FOUND" && String(error.message).includes("@google-cloud/storage")) {
      throw Object.assign(new Error("PROVIDER_PACKAGE_MISSING: install @google-cloud/storage in the server project (npm install @google-cloud/storage)"), {
        code: "PROVIDER_PACKAGE_MISSING",
        cause: error,
      });
    }
    throw error;
  }
  return createGoogleStorageAdapter({ ...options, storage: new sdk.Storage() });
}

function scopedAdapter({ storage, bucket, maxBytes, uploadTimeoutMs, prefix }) {
  return Object.freeze({
    forOwner(owner) {
      if (typeof owner !== "string" || owner.length === 0 || owner.length > 2048 || /[\u0000-\u001f\u007f]/.test(owner)) {
        throw new TypeError("invalid owner");
      }
      const namespace = createHash("sha256").update(owner, "utf8").digest("hex");
      return scopedAdapter({ storage, bucket, maxBytes, uploadTimeoutMs, prefix: `${prefix}${namespace}/` });
    },

    async upload(id, readable, { size, contentType }) {
      const name = objectName(prefix, id);
      if (!Number.isSafeInteger(size) || size < 0 || size > maxBytes) throw new RangeError("object size exceeds configured limit");
      if (typeof contentType !== "string" || contentType.length > 127 || !/^[A-Za-z0-9.+-]+\/[A-Za-z0-9.+-]+$/.test(contentType)) {
        throw new TypeError("invalid content type");
      }
      if (!readable || typeof readable.pipe !== "function") throw new TypeError("readable stream is required");

      let received = 0;
      const exactSize = new Transform({
        transform(chunk, _encoding, callback) {
          if (!(Buffer.isBuffer(chunk) || chunk instanceof Uint8Array || typeof chunk === "string")) {
            callback(new TypeError("object stream must contain bytes"));
            return;
          }
          const bytes = typeof chunk === "string" ? Buffer.from(chunk) : Buffer.from(chunk);
          received += bytes.byteLength;
          if (received > size) callback(new RangeError("stream size exceeds declared size"));
          else callback(null, bytes);
        },
        flush(callback) {
          callback(received === size ? undefined : new RangeError("stream size differs from declared size"));
        },
      });
      const file = storage.bucket(bucket).file(name);
      const sink = file.createWriteStream({
        resumable: true,
        chunkSize: CHUNK_BYTES,
        timeout: uploadTimeoutMs,
        validation: "crc32c",
        contentType,
        metadata: { contentType },
        preconditionOpts: { ifGenerationMatch: 0 },
      });
      await pipeline(readable, exactSize, sink);
      const [metadata] = await file.getMetadata();
      return metadataView(metadata);
    },

    async download(id, writable) {
      const name = objectName(prefix, id);
      if (!writable || typeof writable.write !== "function") throw new TypeError("writable stream is required");
      const bucketRef = storage.bucket(bucket);
      const [metadata] = await bucketRef.file(name).getMetadata();
      const pinnedFile = bucketRef.file(name, { generation: metadata.generation });
      await pipeline(pinnedFile.createReadStream({ validation: "crc32c" }), writable);
      return metadataView(metadata);
    },

    async head(id) {
      const [metadata] = await storage.bucket(bucket).file(objectName(prefix, id)).getMetadata();
      return metadataView(metadata);
    },

    async delete(id) {
      const bucketRef = storage.bucket(bucket);
      const name = objectName(prefix, id);
      let metadata;
      try {
        [metadata] = await bucketRef.file(name).getMetadata();
      } catch (error) {
        if (error?.code === 404) return false;
        throw error;
      }
      await bucketRef.file(name, { generation: metadata.generation }).delete({ ignoreNotFound: true });
      return true;
    },
  });
}

function objectName(prefix, id) {
  if (typeof id !== "string" || !UUID.test(id)) throw new TypeError("invalid object id");
  return `${prefix}${id.toLowerCase()}`;
}

function metadataView(metadata) {
  return {
    size: Number(metadata.size),
    contentType: metadata.contentType ?? "application/octet-stream",
    generation: String(metadata.generation),
    etag: metadata.etag,
  };
}
