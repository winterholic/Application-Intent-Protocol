// contains는 string 값, offset은 정의 budget이 opt-in한 resource의 Query에만 열리는지 컴파일러로 확인한다.
// 서버가 offset 선언 없이 오는 요청을 거부하므로 이 타입은 조기 오류용이고, 최종 판단은 서버다.
import type { FilterItem, Query } from "./generic.ts";

interface C {
  Post: {
    root: true;
    fields: { id: number; title: string };
    traverse: {};
    filterFields: { title: string };
    filter: "title.contains";
    sort: never;
    maxRows: 50;
    maxOffset: 1000;
  };
  Tag: {
    root: true;
    fields: { id: number; name: string };
    traverse: {};
    filterFields: { name: string };
    filter: "name.contains";
    sort: never;
    maxRows: 50;
  };
}

export const okFilter: FilterItem<C, "Post"> = { field: "title", op: "contains", value: "abc" };
// @ts-expect-error contains 값은 string
export const badFilter: FilterItem<C, "Post"> = { field: "title", op: "contains", value: 1 };
// @ts-expect-error 배열 불가
export const badArray: FilterItem<C, "Post"> = { field: "title", op: "contains", value: ["a"] };

export const withOffset: Query<C, "Post"> = { read: "Post", select: ["id"], offset: 20 };
export const noOffset: Query<C, "Post"> = { read: "Post", select: ["id"] };
// @ts-expect-error offset은 number
export const badOffset: Query<C, "Post"> = { read: "Post", select: ["id"], offset: "20" };
// @ts-expect-error budget에 offset을 선언하지 않은 resource는 offset 키를 쓸 수 없다
export const undeclared: Query<C, "Tag"> = { read: "Tag", select: ["id"], offset: 20 };
export const tagOk: Query<C, "Tag"> = { read: "Tag", select: ["id"], filter: [{ field: "name", op: "contains", value: "x" }] };
