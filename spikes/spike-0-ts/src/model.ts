import type { App, Entity, EnumDef, Field, ValueType } from './ir.ts';

// Every entity has an implicit `id: UUID`; it is not stored in Entity.fields.
export const ID_FIELD: Field = { kind: 'scalar', name: 'id', type: 'UUID', optional: false, loc: { line: 0, col: 0 } };

export function snake(s: string): string {
  return s.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();
}

export function q(ident: string): string {
  return `"${ident.replace(/"/g, '""')}"`;
}

export class Model {
  app: App;
  entities = new Map<string, Entity>();
  enums = new Map<string, EnumDef>();

  constructor(app: App) {
    this.app = app;
    for (const e of app.entities) this.entities.set(e.name, e);
    for (const e of app.enums) this.enums.set(e.name, e);
  }

  entity(name: string): Entity {
    const e = this.entities.get(name);
    if (!e) throw new Error(`unknown entity ${name}`);
    return e;
  }

  field(entity: string, name: string): Field | undefined {
    if (name === 'id') return ID_FIELD;
    return this.entities.get(entity)?.fields.find((f) => f.name === name);
  }

  // Fields that occupy a column (everything except reverse `many` relations).
  columns(entity: string): Field[] {
    return [ID_FIELD, ...this.entity(entity).fields.filter((f) => f.kind !== 'many')];
  }

  table(entity: string): string {
    return snake(entity);
  }

  column(f: Field): string {
    return f.kind === 'ref' ? `${snake(f.name)}_id` : snake(f.name);
  }
}

export function fieldType(f: Field): ValueType {
  switch (f.kind) {
    case 'scalar':
      return { k: 'scalar', name: f.type };
    case 'enum':
      return { k: 'enum', name: f.enum };
    case 'ref':
      return { k: 'ref', entity: f.target };
    case 'many':
      return { k: 'many', entity: f.target };
  }
}

export function typeName(t: ValueType | undefined): string {
  if (!t) return '?';
  switch (t.k) {
    case 'scalar':
    case 'enum':
      return t.name;
    case 'ref':
      return t.entity;
    case 'many':
      return `${t.entity}[]`;
    case 'null':
      return 'null';
  }
}
