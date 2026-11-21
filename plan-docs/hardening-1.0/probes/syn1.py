import os,sys; sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import *
B=BASE
T='''resource Todo {
  fields { id: Id; title: Text(1..100); done: Bool; qty: Int(0..1000); owner: Member; secret: Text? }
  rows read when owner = actor
  %s
}
'''
RD='expose read { select id, title; budget { rows 10; depth 1; deadline 1s; cost 100 } }'
cases=[
 ('ok-base','minimal accepted',B+T%RD,'OK'),
 ('unknown-key','unknown keys are errors',B+T%'bogus x','DEF.PARSE_UNKNOWN_KEY'),
 ('budget-missing-cost','budget needs 4 values',B+T%'expose read { select id; budget { rows 10; depth 1; deadline 1s } }','DEF.MISSING_ITEM'),
 ('read-no-budget','expose read w/o budget ok (traverse-only)',B+T%'expose read { select id }','OK'),
 ('apply-no-bulk','expose apply needs bulk maxRows',B+T%'transition t { from true; to done = true; allow owner = actor }\n expose apply t { target id }','DEF.MISSING_ITEM'),
 ('trans-no-from','transition without from',B+T%'transition t { to done = true; allow owner = actor }','DEF.MISSING_ITEM'),
 ('trans-no-allow','transition without allow',B+T%'transition t { from true; to done = true }','DEF.MISSING_ITEM'),
 ('trans-no-effect','transition w/o to/patch/delete',B+T%'transition t { from true; allow owner = actor }',None),
 ('trans-patch-only','patch-only transition ok',B+T%'transition t { from true; allow owner = actor; patch title }','OK'),
 ('trans-delete','delete transition ok',B+T%'transition t { from true; delete; allow owner = actor }','OK'),
 ('trans-delete-to','delete with to rejected',B+T%'transition t { from true; delete; to done = true; allow owner = actor }',None),
 ('trans-delete-patch','delete with patch rejected',B+T%'transition t { from true; delete; patch title; allow owner = actor }',None),
 ('trans-delete-expect','delete with expect rejected',B+T%'transition t { from true; delete; expect title; allow owner = actor }',None),
 ('trans-delete-repeat','delete with repeat unchanged rejected',B+T%'transition t { from true; delete; repeat unchanged; allow owner = actor }',None),
 ('arith-inc','f = f + 1',B+T%'transition t { from true; to qty = qty + 1; allow owner = actor }','OK'),
 ('arith-mul','multiplication rejected',B+T%'transition t { from true; to qty = qty * 2; allow owner = actor }',None),
 ('arith-two','two terms rejected',B+T%'transition t { from true; to qty = qty + 1 + 2; allow owner = actor }',None),
 ('arith-this-self','f = f + this.f is the same as a product, rejected',B+T%'transition t { from true; to qty = qty + this.qty; allow owner = actor }','DEF.ARITH_NOT_ALLOWED'),
 ('input-int-range','Int input range lower bound >=1',B+T%'transition t { input { n: Int(1..100) }; from qty >= input.n; to qty = qty - input.n; allow owner = actor }','OK'),
 ('input-int-norange','Int input needs range',B+T%'transition t { input { n: Int }; from true; to qty = qty - input.n; allow owner = actor }',None),
 ('input-int-zero','Int input lower bound 0 rejected',B+T%'transition t { input { n: Int(0..100) }; from true; to qty = qty - input.n; allow owner = actor }',None),
 ('input-9','input >8 keys rejected',B+T%'transition t { input { a: Int(1..9); b: Int(1..9); c: Int(1..9); d: Int(1..9); e: Int(1..9); f: Int(1..9); g: Int(1..9); h: Int(1..9); i: Int(1..9) }; from true; to qty = qty + input.a; allow owner = actor }',None),
 ('input-text','input Text rejected',B+T%'transition t { input { n: Text }; from true; to qty = qty + 1; allow owner = actor }',None),
 ('dec-lit','Decimal quoted literal ok',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from price >= "1.50"; to price = "0.00"; allow owner = actor } }\n','OK'),
 ('dec-int-lit','Decimal 0 rejected DEF.BAD_VALUE',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = 0; allow owner = actor } }\n','DEF.BAD_VALUE'),
 ('dec-lit-00','Decimal 0.00 rejected DEF.BAD_VALUE',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = 0.00; allow owner = actor } }\n','DEF.BAD_VALUE'),
 ('dec-lead0','Decimal "01" rejected',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = "01.00"; allow owner = actor } }\n','DEF.BAD_VALUE'),
 ('dec-plus','Decimal "+1.00" rejected',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = "+1.00"; allow owner = actor } }\n','DEF.BAD_VALUE'),
 ('dec-scale','Decimal scale overflow rejected',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = "1.005"; allow owner = actor } }\n','DEF.BAD_VALUE'),
 ('dec-intpart','Decimal int-part overflow rejected',B+'resource P { fields { id: Id; price: Decimal(4,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = "123.00"; allow owner = actor } }\n','DEF.BAD_VALUE'),
 ('dec-fewer-scale','Decimal "1.5" (fewer digits) ok',B+'resource P { fields { id: Id; price: Decimal(12,2); owner: Member }\n rows read when owner = actor\n transition z { from true; to price = "1.5"; allow owner = actor } }\n','OK'),
 ('int-neg-range','Int(-1000..1000)',B+'resource P { fields { id: Id; v: Int(-1000..1000); w: Int(-1000..-1); owner: Member }\n rows read when owner = actor }\n','OK'),
 ('int-lo>hi','lo<=hi required',B+'resource P { fields { id: Id; v: Int(5..1); owner: Member }\n rows read when owner = actor }\n',None),
 ('text-neg','Text(-1..5) rejected',B+'resource P { fields { id: Id; v: Text(-1..5); owner: Member }\n rows read when owner = actor }\n',None),
 ('dec-neg','Decimal(-1,2) rejected',B+'resource P { fields { id: Id; v: Decimal(-1,2); owner: Member }\n rows read when owner = actor }\n',None),
 ('budget-neg','budget rows -1 rejected',B+T%'expose read { select id; budget { rows -1; depth 1; deadline 1s; cost 100 } }',None),
 ('op-range-unsafe','op IO range outside JS safe int',B+'operation read f { input { x: Int(1..9007199254740993) }; output { y: Int(1..2) }; allow actor != null; effect none; deadline 2s; implementation "m.f" }\n',None),
 ('op-range-neg-safe','op negative within safe',B+'operation read f { input { x: Int(-9007199254740991..9007199254740991) }; output { y: Int(1..2) }; allow actor != null; effect none; deadline 2s; implementation "m.f" }\n','OK'),
 ('id-in-fields','X.Id in fields -> ID_FIELD_NEEDS_REFERENCE',B+'resource P { fields { id: Id; m: Member.Id } }\n','DEF.ID_FIELD_NEEDS_REFERENCE'),
 ('id-is-ref','id: Item.Id same error',B+'resource Item { fields { id: Id } }\nresource P { fields { id: Item.Id } }\n','DEF.ID_FIELD_NEEDS_REFERENCE'),
 ('id-uuid','id: Uuid ok',B+'resource P { fields { id: Uuid; owner: Member } rows read when owner = actor }\n','OK'),
 ('two-actors','DEF.MULTIPLE_ACTORS',B+'resource Admin { fields { id: Id } }\nactor Admin\n','DEF.MULTIPLE_ACTORS'),
 ('traverse-via','DEF.TRAVERSE_VIA_FORM',B+'''resource Parent { fields { id: Id; owner: Member } rows read when owner = actor
 expose read { select id; traverse kids via owner { select id; limit 5 }; budget { rows 10; depth 2; deadline 1s; cost 100 } } }
''','DEF.TRAVERSE_VIA_FORM'),
 ('cursor-nullable','DEF.CURSOR_NULLABLE_SORT',B+T%'expose read { select id, secret; sort secret, id; budget { rows 10; depth 1; deadline 1s; cost 100; cursor } }','DEF.CURSOR_NULLABLE_SORT'),
 ('bool-sort','Bool sort ok',B+T%'expose read { select id, done; sort done, id; budget { rows 10; depth 1; deadline 1s; cost 100 } }','OK'),
]
run(cases)
