// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = string & { readonly __resource?: R };

export const idWire = "decimal-string-v13";

export interface Contract {
  Inbox: {
    root: true;
    fields: {
      checked: boolean;
      count: number;
      id: Id<"Inbox">;
      member: Id<"Member"> | null;
      title: string;
    };
    traverse: {
    };
    filterFields: {
      checked: boolean;
      count: number;
      id: Id<"Inbox">;
      title: string;
    };
    filter: "checked.eq" | "count.eq" | "count.gte" | "id.eq" | "title.eq";
    sort: "id";
    maxRows: 50;
  };
  WhereOnly: {
    root: true;
    fields: {
      checked: boolean;
      id: Id<"WhereOnly">;
    };
    traverse: {
    };
    filterFields: {
      title: string;
    };
    filter: "title.eq";
    sort: never;
    maxRows: 20;
  };
}

export const applyContractVersion = "typed-apply-v14";
export interface ApplyContract {
  "Inbox.mark": {
    idInput: Id<"Inbox">;
    idOutput: Id<"Inbox">;
    targets: "id" | "where";
    maxRows: 20;
    where: {
      checked: boolean;
      count: number;
      id: Id<"Inbox">;
      title: string;
    };
  };
  "WhereOnly.mark": {
    idInput: Id<"WhereOnly">;
    idOutput: Id<"WhereOnly">;
    targets: "where";
    maxRows: 2;
    where: {
      title: string;
    };
  };
  "WriteOnly.mark": {
    idInput: Id<"WriteOnly">;
    idOutput: Id<"WriteOnly">;
    targets: "id";
    maxRows: 2;
    where: {
    };
  };
}

import type { ApplyBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "ffed6cacc9c64d402f71ee7de5f261828b60f8280ca6f282bb4e0d8a055172c8";
export const contract: ApplyBinding<Contract, ApplyContract> = { fingerprint: "ffed6cacc9c64d402f71ee7de5f261828b60f8280ca6f282bb4e0d8a055172c8", idWire: "decimal-string-v13" };
