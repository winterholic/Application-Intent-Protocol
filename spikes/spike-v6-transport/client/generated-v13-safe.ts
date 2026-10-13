// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = number & { readonly __resource?: R };

export const idWire = "safe-number-v13";

export interface Contract {
  Item: {
    root: true;
    fields: {
      count: number;
      id: Id<"Item">;
      isChecked: boolean;
      label: string;
      member: Id<"Member">;
      parent: Id<"Parent"> | null;
    };
    traverse: {
      parent: { target: "Parent"; select: "id" | "title" };
    };
    filterFields: {
      id: Id<"Item">;
      label: string;
    };
    filter: "id.eq" | "label.eq";
    sort: "id";
    maxRows: 50;
  };
  Parent: {
    root: false;
    fields: {
      id: Id<"Parent">;
      title: string;
    };
    traverse: {
    };
    filterFields: {
    };
    filter: never;
    sort: never;
    maxRows: 0;
  };
}

import type { ContractBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "1569cc710c502ca4cbcb4f32932a3eddae3ae1dbbb60ca0732f85b8e2b946f5d";
export const contract: ContractBinding<Contract> = { fingerprint: "1569cc710c502ca4cbcb4f32932a3eddae3ae1dbbb60ca0732f85b8e2b946f5d" };
