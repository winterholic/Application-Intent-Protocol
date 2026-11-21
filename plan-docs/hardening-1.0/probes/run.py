import subprocess,json,sys,os,tempfile
AIP=os.environ.get('AIP_BIN') or os.path.join(os.environ.get('CARGO_TARGET_DIR','target'),'debug','aip')
BASE='actor Member\nresource Member { fields { id: Id; name: Text } rows read when id = actor }\n'
CASES=tempfile.mkdtemp(prefix='aip-contract-probe-')
def check(text, dev=False, ext='.aip'):
    p=tempfile.NamedTemporaryFile('w',suffix=ext,dir=CASES,delete=False); p.write(text); p.close()
    cmd=[AIP,'service','check',p.name]+(['--dev-checks'] if dev else [])
    r=subprocess.run(cmd,capture_output=True,text=True)
    out=r.stdout.strip() or r.stderr.strip()
    try: j=json.loads(out)
    except Exception: return r.returncode,None,out[:300]
    codes=[]
    if j.get('ok'): codes=['OK']+['adv:'+a['code'] for a in j.get('advisories',[])]
    else: codes=[d['code'] for d in j.get('diagnostics',[])]
    return r.returncode,codes,j
def run(cases):
    for name,claim,text,expect in cases:
        rc,codes,j=check(text)
        got = codes if codes is not None else ['RAW:'+str(j)]
        if expect is None: ok = 'OK' not in got
        elif isinstance(expect,str): ok = expect in got
        else: ok = all(e in got for e in expect)
        print(('PASS' if ok else 'DIFF'),name,'| claim:',claim,'| expect:',expect,'| got:',got)
        if not ok and isinstance(j,dict) and not j.get('ok'):
            for d in j.get('diagnostics',[])[:3]: print('     ',d['code'],d['line'],d['message'][:140])
