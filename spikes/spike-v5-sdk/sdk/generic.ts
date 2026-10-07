// 같은 타입 연산을 앱별 생성 Contract에 적용한다. 공개 계약이 정책 실행을 대신하지는 않는다.
export type ResourceShape = {
  readonly root: boolean;
  readonly fields: Record<string, unknown>;
  readonly traverse: Record<string, { readonly target: string; readonly select: string }>;
  /** 1:N traverse. 정의가 선언한 resource에만 있다. 결과는 배열이고 maxLimit는 정의 상한이다. */
  readonly traverseMany?: Record<string, { readonly target: string; readonly select: string; readonly maxLimit: number }>;
  readonly filterFields: Record<string, unknown>;
  readonly filter: string;
  readonly sort: string;
  readonly maxRows: number;
  /** 정의 budget이 offset을 opt-in했을 때만 있다. */
  readonly maxOffset?: number;
  /** 정의 budget이 cursor를 opt-in했을 때만 있다. */
  readonly cursor?: true;
};
export type ContractShape<C> = { [R in keyof C]: ResourceShape };
export type ContractBinding<C extends ContractShape<C>> = {
  readonly fingerprint: string;
  readonly __contract?: C;
  readonly readDescriptors?: ReadDescriptors<C>;
};
export type IdWireKind = "legacy" | "safe-number-v13" | "decimal-string-v13";
export type ApplyActionShape = {
  readonly idInput: unknown;
  readonly idOutput: unknown;
  readonly targets: "id" | "where";
  readonly where: Record<string, unknown>;
  readonly maxRows: number;
};
export type ApplyContractShape<A> = { [K in keyof A]: ApplyActionShape };
export type ApplyBinding<C extends ContractShape<C>, A extends ApplyContractShape<A>> = ContractBinding<C> & {
  readonly __apply?: A;
  readonly idWire: IdWireKind;
};
type ApplyWhere<A extends ApplyActionShape> = {
  [F in keyof A["where"] & string]: { readonly field: F; readonly op: "eq"; readonly value: A["where"][F] }
}[keyof A["where"] & string];
type ApplyTarget<A extends ApplyActionShape> =
  | ("id" extends A["targets"] ? { readonly ids: readonly A["idInput"][]; readonly where?: never } : never)
  | ("where" extends A["targets"] ? { readonly where: readonly ApplyWhere<A>[]; readonly ids?: never } : never);
export type ApplyRequest<A extends ApplyContractShape<A>, K extends keyof A & string = keyof A & string> = {
  [Name in K]: { readonly apply: Name; readonly target: ApplyTarget<A[Name]> }
}[K];
export type ApplyResult<I> =
  | { readonly ok: true; readonly changed: readonly I[]; readonly unchanged: readonly I[]; readonly tags: readonly string[]; readonly replayed?: boolean; readonly recovered?: boolean }
  | { readonly ok: false; readonly code: string; readonly msg?: string; readonly recovered?: boolean };
export type PendingResult<A extends ApplyContractShape<A>> = ApplyResult<A[keyof A]["idOutput"]> & { readonly key: string };
export type DeepReadonly<T> = T extends string | number | boolean | bigint | symbol | null | undefined
  ? T : T extends object ? { readonly [K in keyof T]: DeepReadonly<T[K]> } : T;

export type Root<C extends ContractShape<C>> = { [R in keyof C]: C[R]["root"] extends true ? R : never }[keyof C];
type Fields<C extends ContractShape<C>, R extends keyof C> = C[R]["fields"];
type Trav<C extends ContractShape<C>, R extends keyof C> = C[R]["traverse"];
type TravMany<C extends ContractShape<C>, R extends keyof C> = C[R] extends { readonly traverseMany: infer M } ? M : {};
type TravManySel<M, K extends keyof M> = M[K] extends { select: infer S } ? S : never;
type TravManyTarget<C extends ContractShape<C>, M, K extends keyof M> = M[K] extends { target: infer T extends keyof C } ? T : never;
type TravSel<C extends ContractShape<C>, R extends keyof C, K extends keyof Trav<C, R>> = Trav<C, R>[K] extends { select: infer S } ? S : never;
type TravTarget<C extends ContractShape<C>, R extends keyof C, K extends keyof Trav<C, R>> = Trav<C, R>[K] extends { target: infer T extends keyof C } ? T : never;

/** select 항목: 공개 필드 이름, 또는 공개 관계 하나와 그 대상의 공개 필드 목록. */
export type SelectItem<C extends ContractShape<C>, R extends keyof C> =
  | (keyof Fields<C, R> & string)
  | { [K in keyof Trav<C, R> & string]: { readonly [P in K]: { readonly select: readonly TravSel<C, R, K>[] } } }[keyof Trav<C, R> & string]
  // 1:N 관계는 select와 (정의 상한 이하의) limit만 받는다. 자식 안의 관계는 타입에서도 열지 않는다.
  | { [K in keyof TravMany<C, R> & string]: { readonly [P in K]: { readonly select: readonly TravManySel<TravMany<C, R>, K>[]; readonly limit?: number } } }[keyof TravMany<C, R> & string];

// filter 값 타입은 select 공개와 별개인 filterFields에서 온다(F03).
type ValueOf<C extends ContractShape<C>, R extends keyof C, F extends string> = F extends keyof C[R]["filterFields"] ? C[R]["filterFields"][F] : never;
// in은 값 배열(서버 상한 50), isNull은 bool이다. 나머지 연산은 필드 값 하나를 받는다.
type OpValue<O extends string, V> = O extends "in" ? readonly V[] : O extends "isNull" ? boolean : V;
export type FilterItem<C extends ContractShape<C>, R extends keyof C> = {
  [K in C[R]["filter"] & string]: K extends `${infer F}.${infer O}` ? { readonly field: F; readonly op: O; readonly value: OpValue<O, ValueOf<C, R, F>> } : never;
}[C[R]["filter"] & string];
export type SortItem<C extends ContractShape<C>, R extends keyof C> = { readonly field: C[R]["sort"] & string; readonly dir?: "asc" | "desc" };

/** offset은 budget이 opt-in한 resource에만 연다. 값 범위(0..=maxOffset)는 타입이 아니라 서버가 검사한다. */
export type OffsetOpt<C extends ContractShape<C>, R extends keyof C> = C[R] extends { readonly maxOffset: number } ? { readonly offset?: number } : {};

/** after는 budget이 cursor를 opt-in한 resource에만 연다. id는 필수이고 정렬 필드는 선택 값이다. 정확한 키 조합은 서버가 요청 sort와 대조한다. */
type CursorFields<C extends ContractShape<C>, R extends keyof C> = {
  readonly id: C[R]["fields"]["id"];
} & {
  readonly [K in Exclude<((C[R]["sort"] & string) | "id") & keyof C[R]["fields"], "id">]?: C[R]["fields"][K];
};
export type CursorOpt<C extends ContractShape<C>, R extends keyof C> = C[R] extends { readonly cursor: true }
  ? { readonly after?: CursorFields<C, R> }
  : {};

export type Query<C extends ContractShape<C>, R extends Root<C>> = {
  readonly read: R;
  readonly select: readonly SelectItem<C, R>[];
  readonly filter?: readonly FilterItem<C, R>[];
  readonly sort?: readonly SortItem<C, R>[];
  readonly limit?: number;
} & OffsetOpt<C, R> & CursorOpt<C, R>;

type U2I<U> = (U extends unknown ? (x: U) => void : never) extends (x: infer I) => void ? I : never;
type Simplify<T> = { [K in keyof T]: T[K] } & {};
type One<C extends ContractShape<C>, R extends keyof C, I> = I extends keyof Fields<C, R> & string
  ? { [P in I]: Fields<C, R>[P] }
  : I extends object
    ? {
        [Rel in keyof I & keyof Trav<C, R> & string]: I[Rel] extends { readonly select: infer A extends readonly unknown[] }
          ? Simplify<RelRow<C, TravTarget<C, R, Rel>, A>> | null
          : never;
      } & {
        // 1:N: 자식 행 배열. 자식이 없거나 정책상 안 보이면 빈 배열이고 null이 아니다.
        [Rel in keyof I & keyof TravMany<C, R> & string]: I[Rel] extends { readonly select: infer A extends readonly unknown[] }
          ? Simplify<RelRow<C, TravManyTarget<C, TravMany<C, R>, Rel>, A>>[]
          : never;
      }
    : never;
// 관계 안 select도 루트와 같은 규칙: tuple은 항목별, 조건부 항목은 union, 길이 미정 배열은 선택적(r5 R5-04).
type OneF<C extends ContractShape<C>, T extends keyof C, F> = F extends keyof Fields<C, T> & string ? { [P in F]: Fields<C, T>[P] } : never;
type AllOfF<C extends ContractShape<C>, T extends keyof C, A extends readonly unknown[]> = A extends readonly [infer H, ...infer Rest] ? OneF<C, T, H> & AllOfF<C, T, Rest> : unknown;
type RelRow<C extends ContractShape<C>, T extends keyof C, A extends readonly unknown[]> = number extends A["length"] ? Partial<U2I<OneF<C, T, A[number]>>> : AllOfF<C, T, A>;
// 고정 tuple은 항목별 교집합을 만든다. 한 항목이 union(조건부 선택)이면 결과도 union으로 남긴다(F01).
type AllOf<C extends ContractShape<C>, R extends keyof C, T extends readonly unknown[]> = T extends readonly [infer H, ...infer Rest] ? One<C, R, H> & AllOf<C, R, Rest> : unknown;
/** 결과 행 타입. 관계는 대상 행 정책으로 가려질 수 있어 null을 포함한다.
 *  길이를 모르는 배열(동적으로 만든 select)은 모든 필드를 선택적으로 본다. */
export type Row<C extends ContractShape<C>, R extends keyof C, S extends readonly unknown[]> = number extends S["length"]
  ? Simplify<Partial<U2I<One<C, R, S[number]>>>>
  : Simplify<AllOf<C, R, S>>;

type IsUnion<T> = [T] extends [U2I<T>] ? false : true;
/** select 형식 검사: 빈 select 금지, 관계 항목은 관계 키 정확히 하나(F02). */
type CheckItem<I> = I extends string ? I : I extends object ? (IsUnion<keyof I> extends true ? never : I) : never;
export type CheckSel<S extends readonly unknown[]> = S extends readonly [] ? never : { [K in keyof S]: CheckItem<S[K]> };

export type ExtensionShape = { readonly input: Record<string, unknown>; readonly output: Record<string, unknown> };
export type ExtensionContractShape<E> = { [K in keyof E]: ExtensionShape };
export type ScalarDescriptor = { readonly type: string; readonly nullable: boolean; readonly values?: readonly string[] };
export type ExtensionDescriptors<E> = { readonly [K in keyof E]: { readonly input: Readonly<Record<string, ScalarDescriptor>>; readonly output: Readonly<Record<string, ScalarDescriptor>> } };
export type ExtensionBinding<C extends ContractShape<C>, A extends ApplyContractShape<A>, E extends ExtensionContractShape<E>> = ApplyBinding<C, A> & {
  readonly __extensions?: E;
  readonly extensions: ExtensionDescriptors<E>;
  readonly readDescriptors: ReadDescriptors<C>;
};

export type WriteExtensionDescriptors<W> = ExtensionDescriptors<W> & {
  readonly [K in keyof W]: { readonly access: readonly string[] };
};
export type PrototypeBinding<C extends ContractShape<C>, A extends ApplyContractShape<A>, E extends ExtensionContractShape<E>, W extends ExtensionContractShape<W>> = ExtensionBinding<C, A, E> & {
  readonly __writes?: W;
  readonly writeExtensions: WriteExtensionDescriptors<W>;
};
export type WriteExtensionResult<O> =
  | { readonly ok: true; readonly output: DeepReadonly<O>; readonly tags: readonly string[]; readonly replayed?: boolean; readonly recovered?: boolean }
  | { readonly ok: false; readonly code: string; readonly msg?: string; readonly recovered?: boolean };


export type ReadDescriptors<C extends ContractShape<C>> = {
  readonly [R in keyof C]: {
    readonly root: boolean;
    readonly maxRows: number;
    readonly fields: Readonly<{ [F in keyof C[R]["fields"]]: ScalarDescriptor }>;
    readonly traverse: Readonly<{ [T in keyof C[R]["traverse"]]: { readonly target: string; readonly select: readonly string[] } }>;
    readonly traverseMany?: Readonly<{ [T in keyof TravMany<C, R>]: { readonly target: string; readonly select: readonly string[]; readonly maxLimit: number } }>;
  };
};
