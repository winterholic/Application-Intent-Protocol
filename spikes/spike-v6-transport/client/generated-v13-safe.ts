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
      member: Id<"Member"> | null;
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
export const contractFingerprint = "107d5957e224607a2fd3c4ad3e0419cafec31506ce69d24a450cd20147b62694";
export const contract: ContractBinding<Contract> = { fingerprint: "107d5957e224607a2fd3c4ad3e0419cafec31506ce69d24a450cd20147b62694" };
