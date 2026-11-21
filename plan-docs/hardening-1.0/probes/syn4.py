import os,sys,re; sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import *
t=open('product/MANAGED-EXECUTION.md',encoding='utf8').read()
blocks=[m.group(1) for m in re.finditer(r'```aip\n(.*?)```',t,re.S)]
for i in (1,2):
    b=blocks[i]
    txt='actor Member\nresource Member { fields { id: Id } rows read when id = actor }\n'+b
    rc,codes,j=check(txt); print('MANAGED block',i,codes[:5])
    if codes and codes[0]!='OK':
        for d in j['diagnostics'][:4]: print('   ',d['code'],d['line'],d['message'][:150])
B=BASE
P='resource Product {\n fields { id: Id; stock: Int(0..1000000); price: Decimal(12,2); other: Int(0..10); hid: Int(0..10); owner: Member }\n rows read when owner = actor\n field hid read when owner = actor\n %s\n}\n'
cases=[
 ('gu-to-input','to f = input.p => DEF.UNSUPPORTED',B+P%'transition t { input { q: Int(1..5) }; from true; to stock = input.q; allow owner = actor }','DEF.UNSUPPORTED'),
 ('gu-repeat-input','input + repeat unchanged => DEF.UNSUPPORTED',B+P%'transition t { input { q: Int(1..5) }; from true; to stock = stock + input.q; repeat unchanged; allow owner = actor }','DEF.UNSUPPORTED'),
 ('gu-policy-input','policy field vs input => POLICY_FIELD_NOT_USABLE_IN_TRANSITION',B+P%'transition t { input { q: Int(1..5) }; from hid >= input.q; to stock = stock + input.q; allow owner = actor }','DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION'),
 ('gu-dec-into-int','Decimal term into Int => TYPE_MISMATCH',B+P%'transition t { from true; to stock = stock + "1.00"; allow owner = actor }','DEF.TYPE_MISMATCH'),
 ('gu-dec-neg-lit','Decimal <=0 literal => ARITH_NOT_ALLOWED',B+P%'transition t { from true; to price = price + "0.00"; allow owner = actor }','DEF.ARITH_NOT_ALLOWED'),
 ('gu-dec-scale-term','Decimal term scale > s rejected',B+P%'transition t { from true; to price = price + "0.005"; allow owner = actor }',None),
 ('gu-int-term-decfield','Int term on Decimal field ok',B+P%'transition t { from true; to price = price + 1; allow owner = actor }','OK'),
 ('gu-nullable-this','nullable this.g rejected',B+'resource P { fields { id: Id; a: Int(0..9); b: Int(0..9)?; owner: Member } rows read when owner = actor\n transition t { from true; to a = a + this.b; allow owner = actor } }\n',None),
 ('gu-nullable-target','nullable target rejected',B+'resource P { fields { id: Id; a: Int(0..9)?; owner: Member } rows read when owner = actor\n transition t { from true; to a = a + 1; allow owner = actor } }\n',None),
 ('gu-effect-update','update effect f = f - input.qty ok',B+'resource Q { fields { id: Id; s: Int(0..100); owner: Member } rows read when owner = actor\n expose read { select id; budget { rows 10; depth 1; deadline 1s; cost 100 } } }\nresource P { fields { id: Id; q: Q; owner: Member } rows read when owner = actor\n transition t { input { n: Int(1..5) }; from true; to owner = actor; allow owner = actor; update Q where id = this.q { s = s - input.n } } }\n','OK'),
]
run(cases)
# ADVISORY_EFFECT_NO_LOWER_BOUND is a dev advisory: it only appears with --dev-checks.
lower=B+'resource Q { fields { id: Id; s: Decimal(12,2); owner: Member } rows read when owner = actor }\nresource P { fields { id: Id; q: Q; owner: Member } rows read when owner = actor\n transition t { input { n: Decimal(12,2) }; from true; to owner = actor; allow owner = actor; update Q where id = this.q { s = s - input.n } } }\n'
print('default check advisories:',check(lower)[1],'| --dev-checks:',check(lower,dev=True)[1])
