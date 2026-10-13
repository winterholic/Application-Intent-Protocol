export { connectTypedExtensions as connect } from "../../spikes/spike-v6-transport/client/typed.ts";
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
  ExtensionBinding, ExtensionContractShape, ScalarDescriptor, ApplyBinding, ApplyRequest, ApplyResult, ContractBinding, PendingResult,
  PrototypeBinding, WriteExtensionDescriptors, WriteExtensionResult,
  OperationBinding, OperationDescriptors,
  WriteBinding, CreateContractShape, ComposeContractShape, BundleRequest, BundleResult, ComposeRequest, ComposeResult, CreatedRow, PendingWrite,
} from "../../spikes/spike-v5-sdk/sdk/generic.ts";
