#!/usr/bin/env node
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import pg from 'pg';
import { generateClient } from './clientgen.ts';
import { compileFile, hasErrors } from './compile.ts';
import { describe } from './contract.ts';
import { formatDiagnostic } from './diagnostics.ts';
import { explainQuery } from './planner.ts';
import { Runtime } from './runtime/runtime.ts';
import { createAipServer } from './runtime/server.ts';
import { generateDDL, migrate } from './schema.ts';

const USAGE = `usage: aip <command> <app.aip> [options]

  check <file> [--json]         parse + static analysis
  ir <file>                     print the typed IR as JSON
  plan <file> [Query]           explain query execution plans
  ddl <file>                    print PostgreSQL DDL
  migrate <file> --reset        drop and recreate the schema (PoC)
  contract <file>               print the machine-readable contract
  gen-client <file> <out.ts>    generate a TypeScript client
  run <file> [--port 4000]      start the HTTP runtime

env: DATABASE_URL (default postgres://localhost/aip_dev)`;

const DATABASE_URL = process.env.DATABASE_URL ?? 'postgres://localhost/aip_dev';

function fail(msg: string): never {
  console.error(msg);
  process.exit(1);
}

async function main() {
  const [cmd, file, ...rest] = process.argv.slice(2);
  if (!cmd || !file || cmd === 'help') fail(USAGE);
  const flag = (name: string) => rest.includes(name);
  const opt = (name: string) => {
    const i = rest.indexOf(name);
    return i >= 0 ? rest[i + 1] : undefined;
  };

  const { analysis, diagnostics } = compileFile(file);
  if (cmd === 'check') {
    if (flag('--json')) {
      console.log(JSON.stringify(diagnostics, null, 2));
    } else {
      for (const d of diagnostics) console.log(formatDiagnostic(d, file));
      const errors = diagnostics.filter((d) => d.severity === 'error').length;
      console.log(`${errors} error(s), ${diagnostics.length - errors} warning(s)`);
    }
    process.exit(hasErrors(diagnostics) ? 1 : 0);
  }
  if (!analysis || hasErrors(diagnostics)) {
    for (const d of diagnostics) console.error(formatDiagnostic(d, file));
    fail('compilation failed');
  }
  for (const d of diagnostics) console.error(formatDiagnostic(d, file));

  switch (cmd) {
    case 'ir':
      console.log(JSON.stringify(analysis.app, null, 2));
      return;
    case 'plan': {
      const only = rest.find((r) => !r.startsWith('--'));
      for (const qr of analysis.app.queries) {
        if (only && qr.name !== only) continue;
        const p = explainQuery(analysis.model, qr);
        console.log(`query ${p.query}  round-trips: ${p.roundTrips}`);
        p.steps.forEach((s, i) => console.log(`  ${i + 1}. ${s.path}  [${s.strategy}]\n     ${s.sql}`));
        console.log();
      }
      return;
    }
    case 'ddl':
      console.log(generateDDL(analysis.model).join(';\n\n') + ';');
      return;
    case 'migrate': {
      if (!flag('--reset')) fail('PoC migrations only support --reset (drops all AIP tables)');
      const client = new pg.Client({ connectionString: DATABASE_URL });
      await client.connect();
      try {
        const ddl = await migrate(client, analysis.model, { reset: true });
        console.log(`migrated ${DATABASE_URL}: ${ddl.length} statements`);
      } finally {
        await client.end();
      }
      return;
    }
    case 'contract':
      console.log(JSON.stringify(describe(analysis), null, 2));
      return;
    case 'gen-client': {
      const out = rest[0] ?? fail('usage: aip gen-client <file> <out.ts>');
      mkdirSync(dirname(out), { recursive: true });
      writeFileSync(out, generateClient(describe(analysis)));
      console.log(`wrote ${out}`);
      return;
    }
    case 'run': {
      const port = Number(opt('--port') ?? 4000);
      const pool = new pg.Pool({ connectionString: DATABASE_URL });
      const server = createAipServer(new Runtime(analysis, pool), { log: true });
      server.listen(port, () => {
        console.log(`aip runtime on http://localhost:${port}  (db ${DATABASE_URL})`);
        console.log('WARNING: dev auth — the actor is read from the x-aip-actor header');
      });
      return;
    }
    default:
      fail(USAGE);
  }
}

main().catch((e) => fail(e instanceof Error ? e.stack ?? e.message : String(e)));
