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

export const readDescriptors = {"Event":{"fields":{"checked":{"nullable":false,"type":"Bool"},"date":{"nullable":false,"type":"Time"},"id":{"nullable":false,"type":"Id"},"link":{"nullable":false,"type":"Url"},"phase":{"nullable":false,"type":"Enum","values":["READY","DONE"]},"title":{"nullable":false,"type":"Text"}},"maxRows":50,"root":true,"traverse":{}}} as const;

export interface ExtensionContract {
}
export const extensionDescriptors = {} as const;

export interface WriteExtensionContract {
  "Event.confirm": {
    input: {
      id: Id<"Event">;
    };
    output: {
      id: Id<"Event">;
      count: number;
    };
  };
}
export const writeExtensionDescriptors = {"Event.confirm":{"access":["Event.mark"],"input":{"id":{"nullable":false,"type":"Id"}},"output":{"count":{"nullable":false,"type":"Int"},"id":{"nullable":false,"type":"Id"}}}} as const;

import type { PrototypeBinding } from "../sdk/index.ts";
export const contractFingerprint = "12f814f3602ed293b84d9d3b2d98f7c7c52785d7b798326d51daa01a1ed80934";
export const contract: PrototypeBinding<Contract, ApplyContract, ExtensionContract, WriteExtensionContract> = { fingerprint: "12f814f3602ed293b84d9d3b2d98f7c7c52785d7b798326d51daa01a1ed80934", idWire: "decimal-string-v13", extensions: extensionDescriptors, readDescriptors: readDescriptors, writeExtensions: writeExtensionDescriptors };
