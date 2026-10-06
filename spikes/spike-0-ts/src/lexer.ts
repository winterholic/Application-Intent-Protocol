import { CompileError } from './diagnostics.ts';
import type { Loc } from './ir.ts';

export type TokKind = 'ident' | 'number' | 'string' | 'punct' | 'eof';

export interface Token {
  kind: TokKind;
  text: string;
  loc: Loc;
}

const PUNCT2 = ['==', '!=', '<=', '>='];
const PUNCT1 = '{}()[]:,.=<>+-*|&!?';

export function lex(src: string): Token[] {
  const toks: Token[] = [];
  let i = 0;
  let line = 1;
  let col = 1;
  const adv = (n: number) => {
    for (let k = 0; k < n; k++) {
      if (src[i] === '\n') {
        line++;
        col = 1;
      } else {
        col++;
      }
      i++;
    }
  };
  while (i < src.length) {
    const c = src[i];
    if (c === ' ' || c === '\t' || c === '\r' || c === '\n') {
      adv(1);
      continue;
    }
    if (c === '/' && src[i + 1] === '/') {
      while (i < src.length && src[i] !== '\n') adv(1);
      continue;
    }
    const loc = { line, col };
    if (/[A-Za-z_]/.test(c)) {
      let j = i;
      while (j < src.length && /[A-Za-z0-9_]/.test(src[j])) j++;
      toks.push({ kind: 'ident', text: src.slice(i, j), loc });
      adv(j - i);
      continue;
    }
    if (/[0-9]/.test(c)) {
      let j = i;
      while (j < src.length && /[0-9]/.test(src[j])) j++;
      toks.push({ kind: 'number', text: src.slice(i, j), loc });
      adv(j - i);
      continue;
    }
    if (c === '"') {
      let j = i + 1;
      while (j < src.length && src[j] !== '"' && src[j] !== '\n') j++;
      if (src[j] !== '"') {
        throw new CompileError([{ severity: 'error', code: 'AIP-E100', message: 'unterminated string', loc }]);
      }
      toks.push({ kind: 'string', text: src.slice(i + 1, j), loc });
      adv(j + 1 - i);
      continue;
    }
    const two = src.slice(i, i + 2);
    if (PUNCT2.includes(two)) {
      toks.push({ kind: 'punct', text: two, loc });
      adv(2);
      continue;
    }
    if (PUNCT1.includes(c)) {
      toks.push({ kind: 'punct', text: c, loc });
      adv(1);
      continue;
    }
    throw new CompileError([{ severity: 'error', code: 'AIP-E100', message: `unexpected character '${c}'`, loc }]);
  }
  toks.push({ kind: 'eof', text: '<eof>', loc: { line, col } });
  return toks;
}
