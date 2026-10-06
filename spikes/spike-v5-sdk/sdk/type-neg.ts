// 음성 대조: 틀린 기대 타입은 컴파일에 실패해야 한다. 이 파일이 통과하면 타입 검사가 공허하다.
import { client, type Transport } from "./aip.ts";
type Equal<X, Y> = (<T>() => T extends X ? 1 : 2) extends <T>() => T extends Y ? 1 : 2 ? true : false;
type Expect<T extends true> = T;
declare const send: Transport;
const aip = client(send);
export async function neg() {
  const rows = await aip.read({ read: "Recruitment", select: ["title"] });
  type _wrong = Expect<Equal<(typeof rows)[number], { title: number }>>;
}
