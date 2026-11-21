import os,sys; sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import *
B=BASE
R='resource Todo {\n  fields { id: Id; title: Text(1..100); done: Bool; qty: Int(0..1000); other: Int(0..1000); amount: Decimal(12,2); owner: Member; secret: Text? }\n  rows read when owner = actor\n  %s\n}\n'
RD='expose read { select id, title; budget { rows 10; depth 1; deadline 1s; cost 100 } }'
F='field secret read when owner = actor\n  '
cases=[
 ('arith-this2','f = f + this.g',B+R%'transition t { from true; to qty = qty + this.other; allow owner = actor }','OK'),
 ('arith-sub-input-dec','Decimal input',B+R%'transition t { input { a: Decimal(12,2) }; from true; to amount = amount - input.a; allow owner = actor }','OK'),
 ('arith-dec-lit','Decimal literal term',B+R%'transition t { from true; to amount = amount + "1.00"; allow owner = actor }','OK'),
 ('arith-neg-lit','negative literal term',B+R%'transition t { from true; to qty = qty + -1; allow owner = actor }',None),
 ('arith-zero-lit','zero literal term rejected (0이 아닌)',B+R%'transition t { from true; to qty = qty + 0; allow owner = actor }',None),
 ('arith-bool','Bool arith rejected',B+R%'transition t { from true; to done = done + 1; allow owner = actor }',None),
 ('input-compose','compose transition w/ input rejected',B+R%'transition t { input { n: Int(1..9) }; from true; to qty = qty + input.n; allow owner = actor }\n expose compose { bulk maxRows 5; sameScope owner; transitions t }',None),
 ('input-in-where','input only in term; not in create value',B+R%'transition t { input { n: Int(1..9) }; from true; to title = "x"; allow owner = actor; create Todo { qty = input.n } }',None),
 ('pol-filter','policy field in filter rejected',B+R%(F+'expose read { select id, secret; filter secret.eq; budget { rows 10; depth 1; deadline 1s; cost 100 } }'),None),
 ('pol-sort','policy field in sort rejected',B+R%(F+'expose read { select id, secret; sort secret; budget { rows 10; depth 1; deadline 1s; cost 100 } }'),None),
 ('pol-select-ok','policy field select ok',B+R%(F+'expose read { select id, secret; budget { rows 10; depth 1; deadline 1s; cost 100 } }'),'OK'),
 ('pol-trans-from','policy field in from',B+R%(F+'transition t { from secret = "a"; to done = true; allow owner = actor }'),'DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION'),
 ('pol-trans-to-value','policy field in to value',B+'resource Todo {\n  fields { id: Id; copy: Text?; owner: Member; secret: Text? }\n  rows read when owner = actor\n  field secret read when owner = actor\n  transition t { from true; to copy = secret; allow owner = actor }\n}\n','DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION'),
 ('pol-trans-repeat','policy field target with repeat unchanged',B+R%(F+'transition t { from true; to secret = "a"; repeat unchanged; allow owner = actor }'),'DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION'),
 ('pol-trans-allow-ok','policy field in allow ok',B+R%(F+'transition t { from true; to done = true; allow secret = "a" }'),'OK'),
 ('pol-trans-write-ok','to write of policy field without repeat ok',B+R%(F+'transition t { from true; to secret = "a"; allow owner = actor }'),'OK'),
 ('upd-many-0','update many maxRows 0 rejected',B+R%'transition t { from true; to done = true; allow owner = actor; update many Todo maxRows 0 where owner = this.owner { done = true } }',None),
 ('upd-many-1001','update many maxRows 1001 rejected',B+R%'transition t { from true; to done = true; allow owner = actor; update many Todo maxRows 1001 where owner = this.owner { done = true } }',None),
 ('trav-limit-0','traverse limit 0 rejected',B+'resource Kid { fields { id: Id; todo: Todo2 } rows read when todo.owner = actor expose read { select id } }\nresource Todo2 { fields { id: Id; owner: Member } rows read when owner = actor expose read { select id; traverse kids via Kid.todo { select id; limit 0 }; budget { rows 10; depth 2; deadline 1s; cost 100 } } }\n',None),
 ('trav-limit-1001','traverse limit 1001 rejected',B+'resource Kid { fields { id: Id; todo: Todo2 } rows read when todo.owner = actor expose read { select id } }\nresource Todo2 { fields { id: Id; owner: Member } rows read when owner = actor expose read { select id; traverse kids via Kid.todo { select id; limit 1001 }; budget { rows 10; depth 2; deadline 1s; cost 100 } } }\n',None),
 ('trav-limit-1000','traverse limit 1000 ok',B+'resource Kid { fields { id: Id; todo: Todo2 } rows read when todo.owner = actor expose read { select id } }\nresource Todo2 { fields { id: Id; owner: Member } rows read when owner = actor expose read { select id; traverse kids via Kid.todo { select id; limit 1000 }; budget { rows 10; depth 2; deadline 1s; cost 100 } } }\n','OK'),
 ('filter-bool-prefix','prefix on Bool rejected',B+R%'expose read { select id, done; filter done.prefix; budget { rows 10; depth 1; deadline 1s; cost 100 } }',None),
 ('filter-int-contains','contains on Int rejected',B+R%'expose read { select id, qty; filter qty.contains; budget { rows 10; depth 1; deadline 1s; cost 100 } }',None),
 ('filter-text-isnull','isNull on non-null Text rejected',B+R%'expose read { select id, title; filter title.like, title.contains, title.prefix, title.in, title.isNull; budget { rows 10; depth 1; deadline 1s; cost 100 } }','DEF.OP_TYPE_MISMATCH'),
 ('filter-all-int','Int: in and isNull are not offered (isNull needs nullable)',B+R%'expose read { select id, qty; filter qty.eq, qty.in, qty.isNull, qty.gte, qty.lte, qty.gt, qty.lt; budget { rows 10; depth 1; deadline 1s; cost 100 } }','DEF.OP_TYPE_MISMATCH'),
 ('filter-op-unknown','unknown op',B+R%'expose read { select id, qty; filter qty.between; budget { rows 10; depth 1; deadline 1s; cost 100 } }',None),
 ('offset-cursor','offset and cursor may both be declared',B+R%'expose read { select id, qty; sort qty, id; budget { rows 10; depth 1; deadline 1s; cost 100; offset 50; cursor } }','OK'),
 ('cursor-no-id','cursor needs id and sort fields in select',B+R%'expose read { select title; sort qty; budget { rows 10; depth 1; deadline 1s; cost 100; cursor } }',None),
 ('deadline-units','deadline ms,s,m',B+R%'expose read { select id; budget { rows 10; depth 1; deadline 500ms; cost 100 } }','OK'),
 ('deadline-m','deadline m',B+R%'expose read { select id; budget { rows 10; depth 1; deadline 2m; cost 100 } }','OK'),
 ('deadline-h','deadline h rejected',B+R%'expose read { select id; budget { rows 10; depth 1; deadline 2h; cost 100 } }',None),
 ('docs-block','docs summary visibility',B+R%'docs { summary "x"; visibility internal }','OK'),
 ('shared-pk','id: Member shared pk',B+'resource Profile { fields { id: Member; bio: Text? } rows read when id = actor }\n','OK'),
 ('enum-ok','enum',B+'enum S { A, B }\n'+R%'','OK'),
 ('dbdefault','default database',B+'resource P { fields { id: Id; n: Int default database; owner: Member } rows read when owner = actor }\n','OK'),
 ('json-type','Json type',B+'resource P { fields { id: Id; j: Json; owner: Member } rows read when owner = actor }\n','OK'),
 ('float','Float none',B+'resource P { fields { id: Id; j: Float; owner: Member } rows read when owner = actor }\n',None),
 ('date-type','Date type',B+'resource P { fields { id: Id; j: Date; k: Time; e: Email; u: Url; owner: Member } rows read when owner = actor }\n','OK'),
]
run(cases)
