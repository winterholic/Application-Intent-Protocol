// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = string & { readonly __resource?: R };

export const idWire = "decimal-string-v13";

export interface Contract {
}

export const applyContractVersion = "typed-apply-v14";
export interface ApplyContract {
  "Order.confirm": {
    idInput: Id<"Order">;
    idOutput: Id<"Order">;
    targets: "id";
    maxRows: 5;
    where: {
    };
  };
  "Product.take": {
    idInput: Id<"Product">;
    idOutput: Id<"Product">;
    input: {
      qty: number; /** 1..100 */
    };
    targets: "id";
    maxRows: 5;
    where: {
    };
  };
}
export const applyInputDescriptors = {"Product.take":{"input":{"qty":{"range":[1,100],"ty":"Int"}}}} as const;

export const bundleLimits = { maxOps: 20, maxRows: 1000, maxSteps: 10 } as const;
export interface CreateContract {
  "Log": { idOutput: Id<"Log">; fields: { note: string; owner: Id<"Member">; }; optional: never };
  "Slot": { idOutput: Id<"Slot">; fields: { owner: Id<"Member">; taken: boolean; }; optional: never };
}
export interface ComposeContract {
  "Order": { idInput: Id<"Order">; idOutput: Id<"Order">; maxRows: 5; transitions: "confirm"; self: false; creates: { "Log": { idOutput: Id<"Log">; fields: { note: string; owner: Id<"Member">; }; optional: never; sources: { "owner": Id<"Member">; }; constants: "note" } } };
  "Room": { idInput: Id<"Room">; idOutput: Id<"Room">; maxRows: 3; transitions: "touch"; self: false; creates: { "Slot": { idOutput: Id<"Slot">; fields: { owner: Id<"Member">; taken: boolean; }; optional: never; sources: { "owner": Id<"Member">; }; constants: "taken" } } };
}

import type { ApplyBinding, WriteBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "b70e40be606d0fac06be2631244a34216990e7eee5e7b3beaa1b82a95510df7a";
export const contract: ApplyBinding<Contract, ApplyContract> & WriteBinding<CreateContract, ComposeContract> = { fingerprint: "b70e40be606d0fac06be2631244a34216990e7eee5e7b3beaa1b82a95510df7a", idWire: "decimal-string-v13" };
