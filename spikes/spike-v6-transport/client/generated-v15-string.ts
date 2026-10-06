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
export const contractFingerprint = "45cc34b65410476821003b8766ce1979a33a9053420f956d2054d536caed3b41";
export const contract: ApplyBinding<Contract, ApplyContract> = { fingerprint: "45cc34b65410476821003b8766ce1979a33a9053420f956d2054d536caed3b41", idWire: "decimal-string-v13" };
