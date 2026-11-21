# 공개 count 계약과 callerFilter 규칙(문법 카드의 aggregate 행, "공개 count 계약" 절).
import os,sys; sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import *
B=BASE
def agg(body, source_policy='owner = actor'):
    return (B+'access ofM = totalOfVisible(Member)\n'
        'resource Item { fields { id: Id; owner: Member; qty: Int(0..100); isAdmin: Bool }\n  rows read when %s\n}\n'%source_policy+
        'resource Q { fields { id: Id; owner: Member } rows read when owner = actor\n  aggregate a: Int { source Item; sourceAccess ofM; groupKey owner; %s release count }\n}\n'%body)
# groupKey는 선언한 resource(Q)를 가리켜야 하므로 Item이 Q를 참조하게 다시 쓴다.
def agg(body, source_policy='q.owner = actor'):
    return (B+'access ofQ = totalOfVisible(Q)\n'
        'resource Q { fields { id: Id; owner: Member }\n  rows read when owner = actor\n  aggregate a: Int { source Item; sourceAccess ofQ; groupKey q; %s release count }\n}\n'%body+
        'resource Item { fields { id: Id; q: Q; qty: Int(0..100); mine: Bool }\n  rows read when %s\n}\n'%source_policy)
cases=[
 ('count-no-where','where 없는 count는 DEF.ADVISORY_PUBLIC_COUNT(--dev-checks 없이)',agg('callerFilter none; rowOutput none;'),'adv:DEF.ADVISORY_PUBLIC_COUNT'),
 ('count-where','where가 있으면 경고 없음',agg('where qty > 0; callerFilter none; rowOutput none;'),'OK'),
 ('oracle-no-where','rows read 원본 + callerFilter read + where 없음 → DEF.CALLER_FILTER_POLICY_REQUIRED',agg('callerFilter read; rowOutput none;'),'DEF.CALLER_FILTER_POLICY_REQUIRED'),
 ('oracle-where-other','정책을 함의하지 않는 where → 거부',agg('where qty > 0; callerFilter read; rowOutput none;'),'DEF.CALLER_FILTER_POLICY_REQUIRED'),
 ('oracle-where-policy','where AND 항에 정책 전체 → 통과',agg('where q.owner = actor and qty > 0; callerFilter read; rowOutput none;'),'OK'),
 ('oracle-where-rewritten','같은 뜻의 다른 식(actor = q.owner)은 거부',agg('where actor = q.owner; callerFilter read; rowOutput none;'),'DEF.CALLER_FILTER_POLICY_REQUIRED'),
 ('oracle-or-branch','정책이 A or B이면 한 가지만 where에 있어도 통과',agg('where mine = true; callerFilter read; rowOutput none;','q.owner = actor or mine = true'),'OK'),
 ('callerfilter-missing','callerFilter 생략 → DEF.MISSING_ITEM',agg('rowOutput none;'),'DEF.MISSING_ITEM'),
 ('sourceaccess-missing','sourceAccess 생략 → DEF.MISSING_ITEM',B+'resource Q { fields { id: Id; owner: Member } rows read when owner = actor\n aggregate a: Int { source Item; groupKey q; callerFilter none; rowOutput none; release count } }\nresource Item { fields { id: Id; q: Q } rows read when q.owner = actor }\n','DEF.MISSING_ITEM'),
 ('no-rows-read-source','원본에 rows read가 없으면 callerFilter read도 정책 요구 없음(where 없는 공개 count 경고만)',B+'access ofQ = totalOfVisible(Q)\nresource Q { fields { id: Id; owner: Member } rows read when owner = actor\n aggregate a: Int { source Item; sourceAccess ofQ; groupKey q; callerFilter read; rowOutput none; release count } }\nresource Item { fields { id: Id; q: Q; qty: Int(0..100) } }\n','adv:DEF.ADVISORY_PUBLIC_COUNT'),
]
run(cases)
