// SDK 캐시 런타임(실험). 서버가 준 읽기 deps와 쓰기 태그로 무효화한다.
export type WriteOutcome = { status: "ok"; changed: string[] } | { status: "unknown" };
type ReadResult = { rows: unknown[]; deps: string[]; maxAgeMs?: number };
type Entry = { rows: readonly unknown[]; deps: string[]; expiresAt: number };
export class ScopeChanged extends Error {
  name = "ScopeChanged";
}

// 캐시와 호출자가 같은 객체를 나눠 갖지 않게 한다. 화면이 고쳐도 캐시는 서버 값 그대로(r5 R5-03).
const deepFreeze = <T>(v: T): T => {
  if (v && typeof v === "object") {
    for (const x of Object.values(v)) deepFreeze(x);
    Object.freeze(v);
  }
  return v;
};
const own = <T>(v: T): T => deepFreeze(structuredClone(v));

export function createCache({ now = () => performance.now() }: { now?: () => number } = {}) {
  let actor: string | null = null;
  const entries = new Map<string, Entry>();
  // resource별 무효화 번호. 읽기 시작 뒤 바뀌었으면 그 응답은 최신이 아니다.
  const epoch = new Map<string, number>();
  let globalEpoch = 0;
  // 결과를 모르는 쓰기가 끝나기 전에는 캐시에 저장하지 않는다(r5 R5-02).
  let unresolved = 0;
  const ep = (r: string) => epoch.get(r) ?? 0;

  async function fetchOnce(fetch: () => Promise<ReadResult>) {
    const startedAt = now();
    const startGlobal = globalEpoch;
    const startActor = actor;
    const snapshot = new Map(epoch);
    const res = await fetch();
    // fetchOnce를 await한 사이에도 쓰기·세션 변경이 끼어들 수 있어 반환·저장 직전에 검사한다.
    const isCurrent = () => {
      if (actor !== startActor) throw new ScopeChanged("읽는 동안 사용자가 바뀜");
      return globalEpoch === startGlobal && res.deps.every((d) => ep(d) === (snapshot.get(d) ?? 0));
    };
    const age = res.maxAgeMs;
    const expiresAt = startedAt + (typeof age === "number" && Number.isFinite(age) && age > 0 ? age : 0);
    return { res, isCurrent, expiresAt };
  }

  return {
    size: () => entries.size,
    invalidate() {
      entries.clear();
      globalEpoch++;
    },
    setActor(next: string | null) {
      if (next !== actor) {
        entries.clear();
        globalEpoch++;
      }
      actor = next;
    },
    async read(query: unknown, fetch: () => Promise<ReadResult>) {
      const key = `${actor}|${JSON.stringify(query)}`;
      const hit = entries.get(key);
      if (hit && now() < hit.expiresAt) return { rows: own(hit.rows), cached: true, stale: false };
      if (hit) entries.delete(key);
      // 읽는 도중 관련 쓰기가 있었으면 그 응답을 돌려주지 않고 다시 읽는다(최대 2번 더).
      for (let i = 0; i < 3; i++) {
        const { res, isCurrent, expiresAt } = await fetchOnce(fetch);
        if (isCurrent()) {
          const stored = unresolved === 0 && now() < expiresAt;
          if (stored) entries.set(key, { rows: own(res.rows), deps: [...res.deps], expiresAt });
          return { rows: own(res.rows), cached: false, stale: false, stored };
        }
      }
      throw new Error("관련 쓰기가 계속 일어나 최신 결과를 얻지 못함");
    },
    onWrite(outcome: WriteOutcome) {
      if (outcome.status === "unknown") {
        // 커밋됐는지 모른다. 전부 버리고, 확인될 때까지 새 결과도 저장하지 않는다.
        unresolved++;
        entries.clear();
        globalEpoch++;
        return;
      }
      for (const r of outcome.changed) epoch.set(r, ep(r) + 1);
      for (const [k, e] of entries) if (e.deps.some((d) => outcome.changed.includes(d))) entries.delete(k);
    },
    /** 상태 조회 등으로 미확정 쓰기의 결과를 확인했을 때. 그 사이 저장 안 한 결과는 다시 읽게 비운다. */
    resolveUnknown() {
      if (unresolved > 0) unresolved--;
      entries.clear();
      globalEpoch++;
    },
  };
}
