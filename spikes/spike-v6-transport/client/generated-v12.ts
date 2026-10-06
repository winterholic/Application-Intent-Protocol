// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.
export type Id<R extends string> = number & { readonly __resource?: R };

export interface Contract {
  Club: {
    root: false;
    fields: {
      id: Id<"Club">;
      logo: string | null;
      name: string;
    };
    traverse: {
    };
    filterFields: {
    };
    filter: never;
    sort: never;
    maxRows: 0;
  };
  MemberAlarm: {
    root: true;
    fields: {
      id: Id<"MemberAlarm">;
      isChecked: boolean;
    };
    traverse: {
    };
    filterFields: {
    };
    filter: never;
    sort: "id";
    maxRows: 100;
  };
  Recruitment: {
    root: true;
    fields: {
      bookmarkCount: number;
      id: Id<"Recruitment">;
      internalNote: string | null;
      periodEnd: string;
      title: string;
      views: number;
    };
    traverse: {
      club: { target: "Club"; select: "id" | "logo" | "name" };
    };
    filterFields: {
      periodEnd: string;
    };
    filter: "periodEnd.gte" | "periodEnd.lte";
    sort: "id" | "periodEnd" | "views";
    maxRows: 50;
  };
}

import type { ContractBinding } from "../../spike-v5-sdk/sdk/generic.ts";
export const contractFingerprint = "5f8527836ee3d4630838ec513a0392b5ddf0023bcd08b5ed5d41285840f2de8e";
export const contract: ContractBinding<Contract> = { fingerprint: "5f8527836ee3d4630838ec513a0392b5ddf0023bcd08b5ed5d41285840f2de8e" };
