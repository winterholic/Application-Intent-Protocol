// 1:N traverse(traverseMany)는 배열로 나온다. select는 정의가 연 자식 필드만, limit는 숫자(상한은 서버가 검사).
// 선언이 없는 resource에는 이 키가 없고 기존 N:1 traverse 타입은 그대로다.
import type { Query, Row, SelectItem } from "./generic.ts";

interface C {
  Post: {
    root: true;
    fields: { id: number; title: string };
    traverse: {};
    traverseMany: { comments: { target: "Comment"; select: "id" | "body" | "secret"; maxLimit: 20 } };
    filterFields: {};
    filter: never;
    sort: never;
    maxRows: 50;
  };
  Comment: {
    root: false;
    fields: { id: number; body: string; secret: string | null; state: string };
    traverse: {};
    filterFields: {};
    filter: never;
    sort: never;
    maxRows: 0;
  };
  Tag: {
    root: true;
    fields: { id: number; name: string };
    traverse: {};
    filterFields: {};
    filter: never;
    sort: never;
    maxRows: 50;
  };
}

export const ok = { read: "Post", select: ["id", { comments: { select: ["id", "body"] as const, limit: 5 } }] } as const satisfies Query<C, "Post">;
type R = Row<C, "Post", typeof ok.select>;
// 자식 필드는 배열 타입이다(null이 아니다). 자식 필드 값은 자식 타입을 따른다.
export const arr: R["comments"] = [{ id: 1, body: "x" }];
export const empty: R["comments"] = [];
// @ts-expect-error 배열이어야 한다
export const notArray: R["comments"] = { id: 1, body: "x" };
// @ts-expect-error null 불가: 자식이 없으면 빈 배열
export const notNull: R["comments"] = null;
// @ts-expect-error 선택하지 않은 필드는 없다
export const noSecret: R["comments"][number]["secret"] = null;
// @ts-expect-error 열리지 않은 자식 필드(state)는 select에 쓸 수 없다
export const badSel: SelectItem<C, "Post"> = { comments: { select: ["state"] } };
// @ts-expect-error 계약에 없는 관계
export const badRel: SelectItem<C, "Post"> = { replies: { select: ["id"] } };
// @ts-expect-error 자식 select 안의 관계(중첩)는 타입에서도 막는다
export const nested: SelectItem<C, "Post"> = { comments: { select: [{ comments: { select: ["id"] } }] } };
// @ts-expect-error 선언이 없는 resource에는 관계가 없다
export const none: SelectItem<C, "Tag"> = { comments: { select: ["id"] } };
// @ts-expect-error limit는 숫자
export const badLimit: SelectItem<C, "Post"> = { comments: { select: ["id"], limit: "5" } };
