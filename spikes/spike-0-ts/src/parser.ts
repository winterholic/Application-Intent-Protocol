// DSL → IR. The grammar is deliberately rigid: each construct has exactly one
// spelling and command/query clauses must appear in a fixed order, so that two
// authors (human or LLM) describing the same intent produce the same text.

import { CompileError } from './diagnostics.ts';
import type {
  App,
  BinOp,
  Command,
  Emit,
  Entity,
  EnumDef,
  Expr,
  Field,
  FieldValue,
  Invariant,
  Load,
  Loc,
  Param,
  PathExpr,
  Policy,
  Query,
  Require,
  ScalarName,
  Selection,
  SelNode,
  Stmt,
  ValueType,
} from './ir.ts';
import { SCALARS } from './ir.ts';
import { lex, type Token } from './lexer.ts';

const COMMAND_CLAUSES = ['idempotent', 'load', 'policy', 'require', 'transaction', 'emit', 'returns'] as const;
const QUERY_CLAUSES = ['from', 'by', 'where', 'sort', 'limit', 'policy', 'select'] as const;

class Parser {
  toks: Token[];
  pos = 0;
  constructor(toks: Token[]) {
    this.toks = toks;
  }

  get tok(): Token {
    return this.toks[this.pos];
  }

  fail(message: string, loc: Loc = this.tok.loc, help?: string): never {
    throw new CompileError([{ severity: 'error', code: 'AIP-E100', message, loc, help }]);
  }

  is(text: string): boolean {
    const t = this.tok;
    return (t.kind === 'punct' || t.kind === 'ident') && t.text === text;
  }

  accept(text: string): boolean {
    if (this.is(text)) {
      this.pos++;
      return true;
    }
    return false;
  }

  expect(text: string): Token {
    if (!this.is(text)) this.fail(`expected '${text}' but found '${this.tok.text}'`);
    return this.toks[this.pos++];
  }

  ident(what = 'identifier'): Token {
    if (this.tok.kind !== 'ident') this.fail(`expected ${what} but found '${this.tok.text}'`);
    return this.toks[this.pos++];
  }

  // ---- top level ----

  app(file: string): App {
    const app: App = { file, actor: null, enums: [], entities: [], queries: [], commands: [] };
    while (this.tok.kind !== 'eof') {
      const kw = this.ident('declaration keyword');
      switch (kw.text) {
        case 'actor': {
          if (app.actor) this.fail('duplicate actor declaration', kw.loc);
          app.actor = { entity: this.ident('entity name').text, loc: kw.loc };
          break;
        }
        case 'enum':
          app.enums.push(this.enumDef(kw.loc));
          break;
        case 'entity':
          app.entities.push(this.entity(kw.loc));
          break;
        case 'query':
          app.queries.push(this.query(kw.loc));
          break;
        case 'command':
          app.commands.push(this.command(kw.loc));
          break;
        default:
          this.fail(`unknown declaration '${kw.text}'`, kw.loc, 'expected one of: actor, enum, entity, query, command');
      }
    }
    return app;
  }

  enumDef(loc: Loc): EnumDef {
    const name = this.ident('enum name').text;
    this.expect('{');
    const values: string[] = [];
    while (!this.accept('}')) values.push(this.ident('enum value').text);
    return { name, values, loc };
  }

  entity(loc: Loc): Entity {
    const name = this.ident('entity name').text;
    this.expect('{');
    const fields: Field[] = [];
    const invariants: Invariant[] = [];
    while (!this.accept('}')) {
      if (this.is('invariant')) {
        const iloc = this.tok.loc;
        this.pos++;
        const iname = this.ident('invariant name').text;
        this.expect(':');
        invariants.push({ name: iname, expr: this.expr(), loc: iloc });
        continue;
      }
      fields.push(this.field());
    }
    return { name, fields, invariants, loc };
  }

  field(): Field {
    const n = this.ident('field name');
    this.expect(':');
    const t = this.ident('type');
    if (this.accept('[')) {
      this.expect(']');
      if (!this.accept('via')) this.fail(`list field '${n.text}' needs 'via <field>'`, t.loc, `write: ${n.text}: ${t.text}[] via <backref field on ${t.text}>`);
      const via = this.ident('backref field').text;
      return { kind: 'many', name: n.text, target: t.text, via, loc: n.loc };
    }
    const optional = this.accept('?');
    if ((SCALARS as readonly string[]).includes(t.text)) {
      return { kind: 'scalar', name: n.text, type: t.text as ScalarName, optional, loc: n.loc };
    }
    // Enum vs entity reference is resolved by the analyzer; `ref` is a placeholder here.
    return { kind: 'ref', name: n.text, target: t.text, optional, loc: n.loc };
  }

  params(): Param[] {
    const params: Param[] = [];
    if (!this.accept('(')) return params;
    if (this.accept(')')) return params;
    do {
      const n = this.ident('parameter name');
      this.expect(':');
      const t = this.ident('parameter type');
      const type: ValueType = (SCALARS as readonly string[]).includes(t.text)
        ? { k: 'scalar', name: t.text as ScalarName }
        : { k: 'enum', name: t.text };
      params.push({ name: n.text, type, loc: n.loc });
    } while (this.accept(','));
    this.expect(')');
    return params;
  }

  // Enforces canonical clause order.
  clause<T extends readonly string[]>(order: T, last: { idx: number }, owner: string): T[number] {
    const t = this.tok;
    const idx = order.indexOf(t.text);
    if (t.kind !== 'ident' || idx < 0) {
      this.fail(`unexpected '${t.text}' in ${owner}`, t.loc, `clauses are: ${order.join(', ')} (in this order)`);
    }
    if (idx < last.idx) {
      this.fail(`'${t.text}' must come before '${order[last.idx]}'`, t.loc, `canonical clause order: ${order.join(' → ')}`);
    }
    last.idx = idx;
    this.pos++;
    return t.text as T[number];
  }

  query(loc: Loc): Query {
    const name = this.ident('query name').text;
    const params = this.params();
    this.expect('{');
    const q: Query = { name, params, entity: '', as: '', by: null, where: null, sort: null, limit: null, policy: null, select: { fields: [] }, loc };
    const last = { idx: -1 };
    let sawSelect = false;
    const seen = new Set<string>();
    while (!this.accept('}')) {
      const cloc = this.tok.loc;
      const c = this.clause(QUERY_CLAUSES, last, `query ${name}`);
      if (seen.has(c)) this.fail(`duplicate '${c}' clause`, cloc);
      seen.add(c);
      switch (c) {
        case 'from':
          q.entity = this.ident('entity name').text;
          this.expect('as');
          q.as = this.ident('alias').text;
          break;
        case 'by':
          q.by = this.expr();
          break;
        case 'where':
          q.where = this.expr();
          break;
        case 'sort': {
          this.expect('by');
          const p = this.path();
          if (p.root !== q.as || p.fields.length !== 1) this.fail(`sort must be '${q.as}.<field>'`, p.loc);
          const dir = this.accept('desc') ? 'desc' : (this.expect('asc'), 'asc');
          q.sort = { field: p.fields[0], dir, loc: cloc };
          break;
        }
        case 'limit': {
          if (this.tok.kind !== 'number') this.fail('limit needs a number');
          q.limit = Number(this.toks[this.pos++].text);
          break;
        }
        case 'policy':
          this.expect(':');
          q.policy = this.policy();
          break;
        case 'select':
          q.select = this.selection();
          sawSelect = true;
          break;
      }
    }
    if (!q.entity) this.fail(`query ${name} has no 'from' clause`, loc);
    if (!sawSelect) this.fail(`query ${name} has no 'select' clause`, loc);
    if (q.by && q.where) this.fail(`query ${name} uses both 'by' and 'where'`, loc, "'by' fetches one row, 'where' fetches a list; pick one");
    return q;
  }

  selection(): Selection {
    this.expect('{');
    const fields: SelNode[] = [];
    while (!this.accept('}')) {
      const n = this.ident('field name');
      fields.push({ name: n.text, sub: this.is('{') ? this.selection() : null, loc: n.loc });
    }
    return { fields };
  }

  command(loc: Loc): Command {
    const name = this.ident('command name').text;
    const params = this.params();
    this.expect('{');
    const c: Command = { name, params, idempotent: false, loads: [], policy: null, requires: [], tx: [], emits: [], returns: null, loc };
    const last = { idx: -1 };
    const once = new Set<string>();
    while (!this.accept('}')) {
      const cloc = this.tok.loc;
      const kw = this.clause(COMMAND_CLAUSES, last, `command ${name}`);
      if (kw !== 'load' && kw !== 'require' && kw !== 'emit') {
        if (once.has(kw)) this.fail(`duplicate '${kw}' clause`, cloc);
        once.add(kw);
      }
      switch (kw) {
        case 'idempotent':
          c.idempotent = true;
          break;
        case 'load':
          c.loads.push(this.load(cloc));
          break;
        case 'policy':
          this.expect(':');
          c.policy = this.policy();
          break;
        case 'require':
          c.requires.push(this.require(cloc));
          break;
        case 'transaction':
          c.tx = this.block();
          break;
        case 'emit':
          c.emits.push(this.emit(cloc));
          break;
        case 'returns':
          c.returns = { name: this.ident('binding name').text, loc: cloc };
          break;
      }
    }
    return c;
  }

  load(loc: Loc): Load {
    const n = this.ident('binding name').text;
    this.expect(':');
    const entity = this.ident('entity name').text;
    this.expect('by');
    const by = this.expr();
    const lock = this.accept('lock');
    return { name: n, entity, by, lock, loc };
  }

  require(loc: Loc): Require {
    const cond = this.expr();
    this.expect('else');
    const code = this.ident('error code');
    if (!/^[A-Z][A-Z0-9_]*$/.test(code.text)) this.fail(`error code '${code.text}' must be UPPER_SNAKE_CASE`, code.loc);
    return { cond, code: code.text, loc };
  }

  emit(loc: Loc): Emit {
    const event = this.ident('event name').text;
    return { event, fields: this.fieldValues(), loc };
  }

  fieldValues(): FieldValue[] {
    this.expect('{');
    const out: FieldValue[] = [];
    if (this.accept('}')) return out;
    do {
      const n = this.ident('field name');
      this.expect(':');
      out.push({ name: n.text, value: this.expr(), loc: n.loc });
    } while (this.accept(','));
    this.expect('}');
    return out;
  }

  block(): Stmt[] {
    this.expect('{');
    const out: Stmt[] = [];
    while (!this.accept('}')) out.push(this.stmt());
    return out;
  }

  stmt(): Stmt {
    const kw = this.ident('statement');
    const loc = kw.loc;
    switch (kw.text) {
      case 'set': {
        const target = this.path();
        this.expect('=');
        return { k: 'set', target, value: this.expr(), loc };
      }
      case 'increment':
      case 'decrement': {
        const target = this.path();
        this.expect('by');
        return { k: 'inc', target, by: this.expr(), sign: kw.text === 'increment' ? 1 : -1, loc };
      }
      case 'create': {
        const entity = this.ident('entity name').text;
        const fields = this.fieldValues();
        const as = this.accept('as') ? this.ident('binding name').text : null;
        return { k: 'create', entity, fields, as, loc };
      }
      case 'each': {
        const source = this.path();
        this.expect('as');
        const as = this.ident('iterator name').text;
        return { k: 'each', source, as, body: this.block(), loc };
      }
    }
    this.fail(`unknown statement '${kw.text}'`, loc, 'statements are: set, increment, decrement, create, each');
  }

  policy(): Policy {
    let l = this.policyAnd();
    while (this.is('|')) {
      const loc = this.tok.loc;
      this.pos++;
      l = { k: 'or', l, r: this.policyAnd(), loc };
    }
    return l;
  }

  policyAnd(): Policy {
    let l = this.policyAtom();
    while (this.is('&')) {
      const loc = this.tok.loc;
      this.pos++;
      l = { k: 'and', l, r: this.policyAtom(), loc };
    }
    return l;
  }

  policyAtom(): Policy {
    const loc = this.tok.loc;
    if (this.accept('(')) {
      const p = this.policy();
      this.expect(')');
      return p;
    }
    const t = this.ident('policy');
    switch (t.text) {
      case 'public':
        return { k: 'public', loc };
      case 'authenticated':
        return { k: 'authenticated', loc };
      case 'owner': {
        this.expect('(');
        const path = this.path();
        this.expect(')');
        return { k: 'owner', path, loc };
      }
      case 'role': {
        this.expect('(');
        const role = this.ident('role').text;
        this.expect(')');
        return { k: 'role', role, loc };
      }
    }
    this.fail(`unknown policy '${t.text}'`, loc, 'policies are: public, authenticated, owner(<path>), role(<ROLE>), combined with | and &');
  }

  // ---- expressions ----

  expr(): Expr {
    return this.orExpr();
  }

  orExpr(): Expr {
    let l = this.andExpr();
    while (this.is('or')) {
      const loc = this.tok.loc;
      this.pos++;
      l = { k: 'bin', op: 'or', l, r: this.andExpr(), loc };
    }
    return l;
  }

  andExpr(): Expr {
    let l = this.notExpr();
    while (this.is('and')) {
      const loc = this.tok.loc;
      this.pos++;
      l = { k: 'bin', op: 'and', l, r: this.notExpr(), loc };
    }
    return l;
  }

  notExpr(): Expr {
    if (this.is('not')) {
      const loc = this.tok.loc;
      this.pos++;
      return { k: 'not', e: this.notExpr(), loc };
    }
    return this.cmpExpr();
  }

  cmpExpr(): Expr {
    const l = this.addExpr();
    const t = this.tok;
    if (t.kind === 'punct' && ['==', '!=', '<', '<=', '>', '>='].includes(t.text)) {
      this.pos++;
      return { k: 'bin', op: t.text as BinOp, l, r: this.addExpr(), loc: t.loc };
    }
    if (this.is('in')) {
      this.pos++;
      this.expect('[');
      const list: Expr[] = [];
      if (!this.is(']')) {
        do list.push(this.addExpr());
        while (this.accept(','));
      }
      this.expect(']');
      return { k: 'in', e: l, list, loc: t.loc };
    }
    return l;
  }

  addExpr(): Expr {
    let l = this.mulExpr();
    while (this.is('+') || this.is('-')) {
      const t = this.toks[this.pos++];
      l = { k: 'bin', op: t.text as BinOp, l, r: this.mulExpr(), loc: t.loc };
    }
    return l;
  }

  mulExpr(): Expr {
    let l = this.unary();
    while (this.is('*')) {
      const t = this.toks[this.pos++];
      l = { k: 'bin', op: '*', l, r: this.unary(), loc: t.loc };
    }
    return l;
  }

  unary(): Expr {
    if (this.is('-')) {
      const loc = this.tok.loc;
      this.pos++;
      return { k: 'neg', e: this.unary(), loc };
    }
    return this.primary();
  }

  primary(): Expr {
    const t = this.tok;
    if (t.kind === 'number') {
      this.pos++;
      return { k: 'lit', value: Number(t.text), loc: t.loc };
    }
    if (t.kind === 'string') {
      this.pos++;
      return { k: 'lit', value: t.text, loc: t.loc };
    }
    if (this.accept('(')) {
      const e = this.expr();
      this.expect(')');
      return e;
    }
    if (t.kind === 'ident') {
      if (t.text === 'true' || t.text === 'false') {
        this.pos++;
        return { k: 'lit', value: t.text === 'true', loc: t.loc };
      }
      if (t.text === 'null') {
        this.pos++;
        return { k: 'lit', value: null, loc: t.loc };
      }
      return this.path();
    }
    this.fail(`expected expression but found '${t.text}'`);
  }

  path(): PathExpr {
    const root = this.ident('name');
    const fields: string[] = [];
    while (this.accept('.')) fields.push(this.ident('field name').text);
    return { k: 'path', root: root.text, fields, loc: root.loc };
  }
}

export function parse(src: string, file = 'app.aip'): App {
  return new Parser(lex(src)).app(file);
}
