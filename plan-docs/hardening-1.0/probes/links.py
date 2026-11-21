import re,os,subprocess,glob
docs=['COMPATIBILITY.md','product/SDK.md','product/LIMITS.md','product/SUPPORT.md','spec/definition-syntax.md','product/GETTING-STARTED.md','product/GENERAL-UPDATE.md','README.md','product/OPERATIONS.md','product/MANAGED-EXECUTION.md','product/INSTALL.md','CHANGELOG.md']
def slug(h):
    h=h.strip().lower()
    h=re.sub(r'[`*_~]','',h) if False else h
    h=re.sub(r'[^\w\- 가-힣]','',h)
    return h.replace(' ','-')
def anchors(path):
    out=set()
    for l in open(path,encoding='utf8'):
        m=re.match(r'#{1,6}\s+(.*)',l)
        if m:
            t=re.sub(r'`','',m.group(1)).strip().lower()
            t=re.sub(r'[^\w\- 가-힣]','',t)
            out.add(t.replace(' ','-'))
    return out
bad=[]
for d in docs:
    t=open(d,encoding='utf8').read()
    base=os.path.dirname(d)
    for m in re.finditer(r'\]\(([^)\s]+)\)',t):
        u=m.group(1)
        if u.startswith(('http','mailto')): continue
        p,_,a=u.partition('#')
        tp=os.path.normpath(os.path.join(base,p)) if p else d
        if not os.path.exists(tp): bad.append((d,u,'missing file')); continue
        if a and tp.endswith('.md') and a not in anchors(tp): bad.append((d,u,'missing anchor'))
    # backticked repo paths
    for m in re.finditer(r'`((?:spikes|crates|product|spec|plan-docs|docs|scripts|porting|conformance|prototype)/[A-Za-z0-9_./\-]+?)(?::\d+)?`',t):
        p=m.group(1)
        if not os.path.exists(p) and not any(c in p for c in '*<>'): bad.append((d,p,'missing path'))
for b in bad: print(b)
print(len(bad),'issues')
