import os,sys,re,glob; sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import *
files=['spec/definition-syntax.md']+glob.glob('product/*.md')+['README.md','COMPATIBILITY.md']+glob.glob('docs/*.md')
for f in files:
    t=open(f,encoding='utf8').read()
    for i,m in enumerate(re.finditer(r'```aip\n(.*?)```',t,re.S)):
        txt=m.group(1)
        line=t[:m.start()].count('\n')+1
        rc,codes,j=check(txt)
        full = 'actor ' in txt
        print(f,'L%d'%line,'actor' if full else 'frag',codes[:4] if codes else j)
