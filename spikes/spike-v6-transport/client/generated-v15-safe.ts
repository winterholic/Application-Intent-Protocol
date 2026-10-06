// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = number & { readonly __resource?: R };

export const idWire = "safe-number-v13";

export interface Contract {
  Event: {
    root: true;
    fields: {
      checked: boolean;
      date: string;
      id: Id<"Event">;
      link: string;
      phase: "READY" | "DONE";
      title: string;
    };
    traverse: {
    };
    filterFields: {
      date: string;
      id: Id<"Event">;
      link: string;
      phase: "READY" | "DONE";
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
      phase: "READY" | "DONE";
      title: string;
    };
  };
}

import type { ApplyBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "b405d53f06ad02a30d3addada30c9490f8d0ac13cfd7dc4d4baed32da6dfa5a4";
export const contract: ApplyBinding<Contract, ApplyContract> = { fingerprint: "b405d53f06ad02a30d3addada30c9490f8d0ac13cfd7dc4d4baed32da6dfa5a4", idWire: "safe-number-v13" };
