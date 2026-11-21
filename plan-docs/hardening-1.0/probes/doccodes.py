import re,glob
def read(p): return open(p,encoding='utf8').read()
reg={}
for area,p in (('DEF','spikes/spike-v1-fixture/src/codes.rs'),('WIRE','spikes/spike-v6-transport/src/wire_codes.rs'),('CLIENT','spikes/spike-v6-transport/src/client_codes.rs')):
    reg[area]=set(re.findall(r'\b[iwc]\(\s*"(%s\.[A-Z0-9_]+)"'%area, read(p)))
allreg=set().union(*reg.values())
poc=set(re.findall(r'"(POC\.[A-Z0-9_]+)"',' '.join(read(p) for p in glob.glob('crates/*/src/**/*.rs',recursive=True))))
files=[f for f in glob.glob('**/*.md',recursive=True) if not f.startswith(('plan-docs/','target','node_modules','spikes/spike-0-ts/node_modules')) and 'node_modules' not in f]
skip={'spec/definition-diagnostics.md','spec/wire-errors.md','spec/client-errors.md'}
bad={}
for f in files:
    if f in skip: continue
    for m in re.finditer(r'\b((?:DEF|WIRE|CLIENT)\.[A-Z][A-Z0-9_]*)\b',read(f)):
        c=m.group(1)
        if c not in allreg: bad.setdefault(c,set()).add(f)
for c,fs in sorted(bad.items()): print(c,sorted(fs)[:6])
