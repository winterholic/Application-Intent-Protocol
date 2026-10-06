// 화면 요청에서 결과 타입이 추론되고, 계약 밖 요청은 컴파일 오류가 되는지 본다.
import { client, type Transport } from "./aip.ts";
import type { Id } from "./contract.ts";

type Equal<X, Y> = (<T>() => T extends X ? 1 : 2) extends <T>() => T extends Y ? 1 : 2 ? true : false;
type Expect<T extends true> = T;
declare const send: Transport;
const aip = client(send);

export async function cases() {
  const rows = await aip.read({
    read: "Recruitment",
    select: ["id", "title", "internalNote", "bookmarkCount", { club: { select: ["name", "logo"] } }],
    filter: [{ field: "periodEnd", op: "gte", value: "2026-10-09T00:00:00Z" }],
    sort: [{ field: "periodEnd", dir: "asc" }],
    limit: 20,
  });
  type R = (typeof rows)[number];
  type _row = Expect<
    Equal<R, { id: Id<"Recruitment">; title: string; internalNote: string | null; bookmarkCount: number; club: { name: string; logo: string | null } | null }>
  >;

  // 화면이 logo를 빼면 결과 타입에서도 빠진다(서버 정의 변경 없음).
  const narrow = await aip.read({ read: "Recruitment", select: ["id", { club: { select: ["name"] } }] });
  type _narrow = Expect<Equal<(typeof narrow)[number], { id: Id<"Recruitment">; club: { name: string } | null }>>;

  // 화면 분기로 고른 select: 결과는 두 형태의 union(F01)
  const cond = Math.random() > 0.5;
  const dyn = await aip.read({ read: "Recruitment", select: ["id", cond ? "title" : "views"] });
  type _dyn = Expect<Equal<(typeof dyn)[number], { id: Id<"Recruitment">; title: string } | { id: Id<"Recruitment">; views: number }>>;

  // 관계 안 조건부 select도 union(r5 R5-04)
  const rel = await aip.read({ read: "Recruitment", select: [{ club: { select: [cond ? "name" : "logo"] } }] });
  type _rel = Expect<Equal<(typeof rel)[number], { club: { name: string } | { logo: string | null } | null }>>;

  // @ts-expect-error 빈 select(F02)
  await aip.read({ read: "Recruitment", select: [] });
  // @ts-expect-error 관계 항목에 키 두 개(F02)
  await aip.read({ read: "Recruitment", select: [{ club: { select: ["name"] }, school: { select: ["id"] } }] });

  // @ts-expect-error status는 공개 select에 없음(닫힌 필드)
  await aip.read({ read: "Recruitment", select: ["status"] });
  // @ts-expect-error 관계 대상의 닫힌 필드
  await aip.read({ read: "Recruitment", select: [{ club: { select: ["school"] } }] });
  // @ts-expect-error 계약에 없는 관계
  await aip.read({ read: "Recruitment", select: [{ school: { select: ["id"] } }] });
  // @ts-expect-error 허용 안 된 filter 연산
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "periodEnd", op: "eq", value: "2026-10-09T00:00:00Z" }] });
  // @ts-expect-error select 가능해도 filter 허용 목록에는 없음
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "title", op: "eq", value: "x" }] });
  // @ts-expect-error filter 값 타입(Time은 문자열)
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "periodEnd", op: "gte", value: 3 }] });
  // @ts-expect-error 정렬 허용 목록 밖
  await aip.read({ read: "Recruitment", select: ["id"], sort: [{ field: "title" }] });
  // @ts-expect-error Club은 관계 대상 전용(루트 조회 불가)
  await aip.read({ read: "Club", select: ["id"] });
}
