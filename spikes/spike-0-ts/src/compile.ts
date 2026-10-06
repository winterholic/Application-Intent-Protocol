import { readFileSync } from 'node:fs';
import { analyze, type Analysis } from './analyzer.ts';
import { CompileError, type Diagnostic } from './diagnostics.ts';
import { parse } from './parser.ts';

export function compileSource(src: string, file: string): { analysis: Analysis | null; diagnostics: Diagnostic[] } {
  try {
    const analysis = analyze(parse(src, file));
    return { analysis, diagnostics: analysis.diagnostics };
  } catch (e) {
    if (e instanceof CompileError) return { analysis: null, diagnostics: e.diagnostics };
    throw e;
  }
}

export function compileFile(file: string) {
  return compileSource(readFileSync(file, 'utf8'), file);
}

export function hasErrors(diags: Diagnostic[]): boolean {
  return diags.some((d) => d.severity === 'error');
}
