// in은 값 배열, isNull은 bool을 받는지 컴파일러로 확인한다. @ts-expect-error 줄은 오류가 나야 통과한다.
import type { FilterItem } from "./generic.ts";

type Id<R extends string> = number & { readonly __resource?: R };
interface C {
  Comment: {
    root: true;
    fields: { id: Id<"Comment"> };
    traverse: {};
    filterFields: { post: Id<"Post">; status: "OPEN" | "HIDDEN"; parent: Id<"Comment"> };
    filter: "post.eq" | "status.in" | "parent.isNull";
    sort: never;
    maxRows: 50;
  };
}
type F = FilterItem<C, "Comment">;

export const ok: readonly F[] = [
  { field: "post", op: "eq", value: 7 },
  { field: "status", op: "in", value: ["OPEN", "HIDDEN"] },
  { field: "parent", op: "isNull", value: true },
];
// @ts-expect-error in은 배열만
export const notArray: F = { field: "status", op: "in", value: "OPEN" };
// @ts-expect-error 배열 원소도 필드 타입을 따른다
export const badItem: F = { field: "status", op: "in", value: ["DELETED"] };
// @ts-expect-error isNull은 bool만
export const badNull: F = { field: "parent", op: "isNull", value: 3 };
// @ts-expect-error 계약에 없는 연산
export const notAllowed: F = { field: "post", op: "in", value: [7] };
