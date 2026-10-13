// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = string & { readonly __resource?: R };

export const idWire = "decimal-string-v13";

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
export const contractFingerprint = "5112b9a2771420056a5e77fa5ff4365973bcccf9a909cd7cd9cbb72710bc968d";
export const contract: ContractBinding<Contract> = { fingerprint: "5112b9a2771420056a5e77fa5ff4365973bcccf9a909cd7cd9cbb72710bc968d" };
