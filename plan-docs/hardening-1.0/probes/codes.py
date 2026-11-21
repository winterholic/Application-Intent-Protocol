import re,glob,os,sys
def read(p):
    try: return open(p,encoding='utf8').read()
    except: return ''
def srcs(excl):
    out={}
    for p in glob.glob('spikes/*/src/**/*.rs',recursive=True)+glob.glob('crates/*/src/**/*.rs',recursive=True):
        if p in excl: continue
        out[p]=read(p)
    return out
for area,reg in (('DEF','spikes/spike-v1-fixture/src/codes.rs'),('WIRE','spikes/spike-v6-transport/src/wire_codes.rs'),('CLIENT','spikes/spike-v6-transport/src/client_codes.rs')):
    rt=read(reg)
    registered=set(re.findall(r'"(%s\.[A-Z0-9_]+)"'%area, rt))
    # first string in each entry only: entries begin with i( or w( or c(
    ents=set(re.findall(r'\b[iwc]\(\s*"(%s\.[A-Z0-9_]+)"'%area, rt))
    S=srcs({reg})
    emitted={}
    for p,t in S.items():
        for m in re.finditer(r'"(%s\.[A-Z0-9_]+)"'%area,t):
            emitted.setdefault(m.group(1),set()).add(p)
    # also in TS/py SDK
    other={}
    for p in glob.glob('spikes/spike-v6-transport/client/*.ts')+glob.glob('product/sdk/**/*.ts',recursive=True)+glob.glob('product/python/aip_sdk/*.py'):
        t=read(p)
        for m in re.finditer(r'["\'](%s\.[A-Z0-9_]+)["\']'%area,t):
            other.setdefault(m.group(1),set()).add(p)
    print('==',area,'registered',len(ents),'distinct in reg text',len(registered))
    print(' emitted-not-registered (src):',sorted(set(emitted)-ents))
    print(' emitted-not-registered (sdk):',sorted(set(other)-ents))
    print(' registered-never-emitted in rust src:',sorted(ents-set(emitted)))
    print(' registered-never-referenced anywhere:',sorted(ents-set(emitted)-set(other)))
