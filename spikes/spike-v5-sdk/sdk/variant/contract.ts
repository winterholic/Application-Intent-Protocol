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
  Recruitment: {
    root: true;
    fields: {
      bookmarkCount: number;
      id: Id<"Recruitment">;
      internalNote: string | null;
      title: string;
      views: number;
    };
    traverse: {
      club: { target: "Club"; select: "id" | "logo" | "name" };
    };
    filterFields: {
      id: Id<"Recruitment"> | `${number}`;
      periodEnd: string;
    };
    filter: "id.eq" | "periodEnd.gte" | "periodEnd.lte";
    sort: "id" | "periodEnd" | "views";
    maxRows: 50;
  };
}
