// 시나리오 공통 도구. 결과는 `RESULT PASS|FAIL <이름> :: <상세>` 줄로 runner에 전달한다.
import { spawnSync } from "node:child_process";

export const addr = process.env.AIP_ADDR!;
export const tokens: Record<string, string> = JSON.parse(process.env.AIP_TOKENS!);
export const schema = process.env.AIP_SCHEMA!;
export const baseUrl = `http://${addr}`;

/** 서비스를 거치지 않고 PostgreSQL을 직접 읽어 SDK 결과를 교차 확인한다. */
export function sql(query: string): string {
  const r = spawnSync("psql", [process.env.AIP_DB!, "-X", "-v", "ON_ERROR_STOP=1", "-At", "-c", query], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`psql 실패: ${r.stderr}`);
  return r.stdout.trim();
}

export async function scenario(name: string, body: () => Promise<void> | void): Promise<void> {
  try {
    await body();
    console.log(`RESULT PASS ${name}`);
  } catch (error) {
    const text = error instanceof Error ? error.message : String(error);
    console.log(`RESULT FAIL ${name} :: ${text.replace(/\s+/gu, " ").slice(0, 400)}`);
  }
}
