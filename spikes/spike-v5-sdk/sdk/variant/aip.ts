// 기존 V5 fixture 호출은 앱별 타입 core의 편의 adapter를 사용한다.
import type { Contract } from "./contract.ts";
import type * as G from "../generic.ts";

export type Root = G.Root<Contract>;
export type SelectItem<R extends keyof Contract> = G.SelectItem<Contract, R>;
export type FilterItem<R extends keyof Contract> = G.FilterItem<Contract, R>;
export type SortItem<R extends keyof Contract> = G.SortItem<Contract, R>;
export type Query<R extends Root> = G.Query<Contract, R>;
export type Row<R extends keyof Contract, S extends readonly unknown[]> = G.Row<Contract, R, S>;
export type CheckSel<S extends readonly unknown[]> = G.CheckSel<S>;

export type Transport = (q: unknown) => Promise<unknown[]>;

export function client(send: Transport) {
  return {
    read<R extends Root, const S extends readonly SelectItem<R>[]>(q: {
      readonly read: R;
      readonly select: S & CheckSel<S>;
      readonly filter?: readonly FilterItem<R>[];
      readonly sort?: readonly SortItem<R>[];
      readonly limit?: number;
    } & G.OffsetOpt<Contract, R> & G.CursorOpt<Contract, R>): Promise<Row<R, S>[]> {
      return send(q) as Promise<Row<R, S>[]>;
    },
  };
}
