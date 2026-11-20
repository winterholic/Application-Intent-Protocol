import { connectTypedExtensions } from "../../spikes/spike-v6-transport/client/typed.ts";
import { SDK_IDENTITY, SDK_PROTOCOL } from "./version.ts";

export { SDK_IDENTITY, SDK_PROTOCOL, SDK_VERSION } from "./version.ts";

// Every request carries `x-aip-sdk: typescript/<version>`. The server only logs it for diagnostics and never rejects on it,
// so an old or unknown SDK is served as before. Headers given as a plain object or as a Headers instance both work.
function declareSdk(fetchImpl: typeof fetch): typeof fetch {
  return (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("x-aip-sdk", SDK_IDENTITY);
    return fetchImpl(input, { ...init, headers: Object.fromEntries(headers.entries()) });
  };
}

/**
 * Compares the `protocol` a server reports (the `protocol` field of `/capabilities` or of the contract document) with the
 * protocol this SDK speaks. Returns a warning text when the majors differ and `undefined` otherwise, including when the server
 * reports nothing (older engines omit it). It never throws: the contract fingerprint remains the compatibility check. The typed
 * client does not call `/capabilities` itself, so a caller that reads it passes the value here.
 */
export function protocolWarning(serverProtocol: unknown, sdkProtocol: string = SDK_PROTOCOL): string | undefined {
  if (serverProtocol === undefined || serverProtocol === null) return undefined;
  const major = (value: unknown) => (typeof value === "string" ? /^aip-protocol\/(\d+)\.\d+$/u.exec(value)?.[1] : undefined);
  if (major(serverProtocol) !== undefined && major(serverProtocol) === major(sdkProtocol)) return undefined;
  const shown = typeof serverProtocol === "string" ? serverProtocol.slice(0, 40) : typeof serverProtocol;
  return `server protocol ${JSON.stringify(shown)} has a different major than this SDK (${sdkProtocol}); use the SDK and binding of the server's release`;
}

export const connect: typeof connectTypedExtensions = (base, token, binding, fetchImpl = fetch) =>
  connectTypedExtensions(base, token, binding, declareSdk(fetchImpl));
export { createInfrastructureClient, readSse } from "../../spikes/spike-v6-transport/client/infrastructure.ts";
export { connectEventWebSocket } from "../../spikes/spike-v6-transport/client/websocket.ts";
export { createRagClient } from "../../spikes/spike-v6-transport/client/rag.ts";
export type { RagCitation, RagHit, RagAnswer, RagSearchOptions } from "../../spikes/spike-v6-transport/client/rag.ts";
export type { BrowserEventSocket, BrowserEventSocketOptions, EventSubscription, RealtimeEvent } from "../../spikes/spike-v6-transport/client/websocket.ts";
export type {
  DeadLetter, DeadLetterPage, InfrastructureClient, ManagedSend, Message, MessageLease, MultipartPart, MultipartUpload,
  ObjectMethod, SignedObject, SignedPart, SseEvent,
} from "../../spikes/spike-v6-transport/client/infrastructure.ts";
export { WriteUnsettled } from "../../spikes/spike-v6-transport/client/transport.ts";
export type { EnqueueOptions, EventPage, FileMetadata, JobReceipt, JobResult, JobStatus, RetrievedFile } from "../../spikes/spike-v6-transport/client/jobs.ts";
export type {
  RetryGuidance, ExtensionBinding, ExtensionContractShape, ScalarDescriptor, ApplyBinding, ApplyRequest, ApplyResult, ContractBinding, PendingResult,
  PrototypeBinding, WriteExtensionDescriptors, WriteExtensionResult,
  OperationBinding, OperationDescriptors,
  WriteBinding, CreateContractShape, ComposeContractShape, BundleRequest, BundleResult, ComposeRequest, ComposeResult, CreatedRow, PendingWrite,
} from "../../spikes/spike-v5-sdk/sdk/generic.ts";
