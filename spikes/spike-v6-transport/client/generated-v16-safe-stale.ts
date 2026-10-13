// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = number & { readonly __resource?: R };

export const idWire = "safe-number-v13";

export interface Contract {
}

export const applyContractVersion = "typed-apply-v14";
export interface ApplyContract {
  "Project.archive": {
    idInput: Id<"Project">;
    idOutput: Id<"Project">;
    targets: "id";
    maxRows: 5;
    where: {
    };
  };
}

export const bundleLimits = { maxOps: 20, maxRows: 1000, maxSteps: 10 } as const;
export interface CreateContract {
  "Link": { idOutput: null; fields: { member: Id<"Member">; project: Id<"Project">; }; optional: never };
  "Log": { idOutput: Id<"Log">; fields: { name: string; note: string | null; owner: Id<"Member">; }; optional: "note" };
  "Task": { idOutput: Id<"Task">; fields: { author: Id<"Member">; points: number; project: Id<"Project">; title: string; }; optional: never };
}
export interface ComposeContract {
  "Project": { idInput: Id<"Project">; idOutput: Id<"Project">; maxRows: 1; transitions: "archive"; self: false; creates: { "Log": { idOutput: Id<"Log">; fields: { name: string; note: string | null; owner: Id<"Member">; }; optional: "note"; sources: { "owner": Id<"Member">; "name": string; }; constants: "name" | "note" } } };
}

import type { ApplyBinding, WriteBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "5572a7eadc1f67ca57f7efe8d9cd42c19baf6c302dfd9d6f64e1315a29f5e9c5";
export const contract: ApplyBinding<Contract, ApplyContract> & WriteBinding<CreateContract, ComposeContract> = { fingerprint: "5572a7eadc1f67ca57f7efe8d9cd42c19baf6c302dfd9d6f64e1315a29f5e9c5", idWire: "safe-number-v13" };
