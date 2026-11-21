import os,sys; sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import *
B=BASE
# filter op matrix by type
types={'Id':'id2: Id2ref','Int':'v: Int(0..9)','IntN':'v: Int(0..9)?','Text':'v: Text','TextN':'v: Text?','Bool':'v: Bool','BoolN':'v: Bool?','Dec':'v: Decimal(12,2)','Time':'v: Time','Date':'v: Date','Email':'v: Email','Url':'v: Url','Enum':'v: St','Ref':'v: Member','RefN':'v: Member?','Uuid':'v: Uuid2'}
ops=['eq','in','isNull','gte','lte','gt','lt','prefix','contains','like']
print('type ops accepted:')
for t,decl in types.items():
    if t in('Id','Uuid'): continue
    acc=[]
    for op in ops:
        txt=B+'enum St { A, B }\nresource P { fields { id: Id; %s; owner: Member } rows read when owner = actor\n expose read { select id, v; filter v.%s; budget { rows 10; depth 1; deadline 1s; cost 100 } } }\n'%(decl,op)
        rc,codes,j=check(txt)
        if codes==['OK']: acc.append(op)
    print(' ',t,acc)
# id filter
for op in ops:
    txt=B+'resource P { fields { id: Id; owner: Member } rows read when owner = actor\n expose read { select id; filter id.%s; budget { rows 10; depth 1; deadline 1s; cost 100 } } }\n'%op
    rc,codes,j=check(txt); print('  id.%s'%op,codes)
