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
      date: never;
      id: Id<"Event">;
      link: never;
      phase: never;
      title: string;
    };
  };
}

import type {ApplyBinding} from '../../spike-v5-sdk/sdk/generic.ts';
export const contract: ApplyBinding<Contract, ApplyContract> = {fingerprint:'bcf514bfef1f6610f5f6e5ae3de0c283e0a1006dd94d238eca3d4b5a8c3beafd',idWire:'safe-number-v13'};