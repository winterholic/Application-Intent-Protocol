"""Prepare immutable ORIGINAL UI/backend copies and local-only Vite adapters.

Never reads .env, edits original repositories, or installs packages.
"""
import argparse
import json
from pathlib import Path
import subprocess
from facereview_runner import safe_extract


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--facereview-root',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parent.parent
    inventory=json.loads((root/'plan-docs/porting/facereview-inventory.json').read_text())
    output=args.out.resolve();output.mkdir(parents=True,exist_ok=False)
    paths={
        'backend':['app','common','tests','requirements.txt'],
        'user':['src','public','package.json','yarn.lock','.yarn','index.html','tsconfig.json','vite.config.ts','vercel.json'],
        'admin':['src','public','package.json','package-lock.json','index.html','tsconfig.json','tsconfig.app.json','tsconfig.node.json','vite.config.ts'],
    }
    for repo in inventory['repositories']:
        role=repo['role'];destination=output/role;destination.mkdir()
        archive=subprocess.check_output(['git','-C',str(args.facereview_root.resolve()/repo['name']),
                                         'archive',repo['sha'],'--',*paths[role]])
        safe_extract(archive,destination)
        if (destination/'.env').exists(): raise RuntimeError('Unexpected .env')
    (output/'user/aip-baseline.config.mjs').write_text(
        "import factory from './vite.config.ts';\n"
        "const config=factory({mode:'baseline',command:'serve'});\n"
        "export default {...config,server:{...config.server,host:'127.0.0.1',port:23173,strictPort:true,allowedHosts:['host.docker.internal'],proxy:{'/api':{target:'http://127.0.0.1:23175',changeOrigin:true,cookieDomainRewrite:''},'/socket.io':{target:'http://127.0.0.1:23175',changeOrigin:true,ws:true}}}};\n")
    (output/'admin/aip-baseline.config.mjs').write_text(
        "import config from './vite.config.ts';\n"
        "export default {...config,server:{host:'127.0.0.1',port:23174,strictPort:true,allowedHosts:['host.docker.internal'],proxy:{'/api':{target:'http://127.0.0.1:23175',changeOrigin:true,cookieDomainRewrite:''}}}};\n")
    (output/'source-revisions.json').write_text(json.dumps({r['role']:r['sha'] for r in inventory['repositories']},indent=2)+'\n')
    print(json.dumps({'prepared':['backend','user','admin'],'production_fallback':False,'out':str(output)}))


if __name__=='__main__':
    main()
