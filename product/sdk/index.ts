import { connectTypedExtensions } from "../../spikes/spike-v6-transport/client/typed.ts";
import { SDK_IDENTITY } from "./version.ts";

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
