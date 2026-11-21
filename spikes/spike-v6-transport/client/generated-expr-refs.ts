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
  "Order": { idOutput: Id<"Order">; fields: { owner: Id<"Member">; status: string; }; optional: never };
  "OrderLine": { idOutput: Id<"OrderLine">; fields: { order: Id<"Order">; owner: Id<"Member">; product: Id<"Product">; qty: number; }; optional: never };
}

import type { ApplyBinding, WriteBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "24f28afe8a8f51a8be2280120e82bc00910bc213950aefda20fc9ebac0f05be9";
export const contract: ApplyBinding<Contract, ApplyContract> & WriteBinding<CreateContract, {}> = { fingerprint: "24f28afe8a8f51a8be2280120e82bc00910bc213950aefda20fc9ebac0f05be9", idWire: "decimal-string-v13" };
