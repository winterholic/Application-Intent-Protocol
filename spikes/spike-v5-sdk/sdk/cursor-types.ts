// after(keyset cursor)는 budget이 cursor를 opt-in한 resource의 Query에만 열리고, 값은 해당 필드의 타입을 따른다.
// 키 집합이 요청 sort와 일치하는지는 서버가 검사한다(타입은 조기 오류용).
import type { Query } from "./generic.ts";

interface C {
  Post: {
    root: true;
    fields: { id: number; views: number; title: string };
    traverse: {};
    filterFields: {};
    filter: never;
    sort: "id" | "title";
    maxRows: 50;
    cursor: true;
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

export const ok: Query<C, "Post"> = { read: "Post", select: ["id"], sort: [{ field: "title", dir: "desc" }], after: { title: "post", id: 9 } };
export const first: Query<C, "Post"> = { read: "Post", select: ["id"] };
export const defaultIdCursor: Query<C, "Post"> = { read: "Post", select: ["id"], after: { id: 9 } };
export const explicitIdCursor: Query<C, "Post"> = { read: "Post", select: ["id"], sort: [{ field: "id" }], after: { id: 9 } };
// @ts-expect-error 값 타입은 필드 타입을 따른다
export const badValue: Query<C, "Post"> = { read: "Post", select: ["id"], after: { id: "9" } };
// @ts-expect-error sort 허용 목록 밖 필드(views)는 cursor 키가 될 수 없다
export const badKey: Query<C, "Post"> = { read: "Post", select: ["id"], after: { views: 5, id: 9 } };
// @ts-expect-error after title follows the field's string value type
export const badTitleValue: Query<C, "Post"> = { read: "Post", select: ["id"], after: { title: 5, id: 9 } };
// @ts-expect-error id 누락
export const noId: Query<C, "Post"> = { read: "Post", select: ["id"], after: { views: 5 } };
// @ts-expect-error cursor를 선언하지 않은 resource는 after 키를 쓸 수 없다
export const undeclared: Query<C, "Tag"> = { read: "Tag", select: ["id"], after: { id: 1 } };
