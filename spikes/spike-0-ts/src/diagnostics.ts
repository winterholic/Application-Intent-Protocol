import type { Loc } from './ir.ts';

export type Severity = 'error' | 'warning';

export interface Diagnostic {
  severity: Severity;
  code: string;
  message: string;
  loc: Loc;
  help?: string;
}

export class CompileError extends Error {
  diagnostics: Diagnostic[];
  constructor(diagnostics: Diagnostic[]) {
    super(diagnostics.map((d) => `${d.code}: ${d.message}`).join('\n'));
    this.diagnostics = diagnostics;
  }
}

export function formatDiagnostic(d: Diagnostic, file: string): string {
  const head = `${d.severity} ${d.code} ${file}:${d.loc.line}:${d.loc.col}  ${d.message}`;
  return d.help ? `${head}\n  help: ${d.help}` : head;
}
