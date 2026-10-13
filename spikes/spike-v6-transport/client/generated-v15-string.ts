// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = string & { readonly __resource?: R };

export const idWire = "decimal-string-v13";

export interface Contract {
  Event: {
    root: true;
    fields: {
      checked: boolean;
      date: string;
      id: Id<"Event">;
      link: string;
      phase: "DONE" | "READY";
      title: string;
    };
    traverse: {
    };
    filterFields: {
      date: string;
      id: Id<"Event">;
      link: string;
      phase: "DONE" | "READY";
      title: string;
    };
    filter: "date.eq" | "date.gte" | "id.eq" | "link.eq" | "phase.eq" | "title.eq" | "title.prefix";
    sort: "id";
    maxRows: 50;
  };
}

export const applyContractVersion = "typed-apply-v14";
export interface ApplyContract {
  "Event.mark": {
    idInput: Id<"Event">;
    idOutput: Id<"Event">;
    targets: "id" | "where";
    maxRows: 20;
    where: {
      date: string;
      id: Id<"Event">;
      link: string;
      phase: "DONE" | "READY";
      title: string;
    };
  };
}

import type { ApplyBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "34f597dd89a90037ca61541c729b2d0f9e298335ff87bd2c1897d0525c69ba77";
export const contract: ApplyBinding<Contract, ApplyContract> = { fingerprint: "34f597dd89a90037ca61541c729b2d0f9e298335ff87bd2c1897d0525c69ba77", idWire: "decimal-string-v13" };
