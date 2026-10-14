"""Browser baseline helpers. This does not port FaceReview to AIP."""
from urllib.parse import urlsplit
import argparse
import json
from pathlib import Path
PLANNED = {('user',label) for label in ['login','main','search','bookmarks','mypage','profile','watch-provider-error-state']} | {('admin',label) for label in ['login','dashboard','users','videos','requests','comments']}


def check_browser_report(report):
    from facereview_oracle import sensitive_fields
    errors=sensitive_fields(report)
    if report.get('scope')!='original-browser' or report.get('aip_ported') is not False or report.get('ml')!='not-executed' or report.get('complete') is not False:
        errors.append('Scope must remain partial original-browser with ML/AIP unexecuted')
    cases=report.get('cases',[])
    pairs=[(c.get('role'),c.get('label')) for c in cases]
    if len(pairs)!=len(PLANNED) or set(pairs)!=PLANNED or any(c.get('result')!='pass' for c in cases):
        errors.append('Planned browser cases incomplete, duplicated or failed')
    if not report.get('network'):
        errors.append('No real network evidence')
    for row in report.get('network',[]):
        if not allowed_request(row.get('origin','')+row.get('path','')):
            errors.append('Non-owned response in network evidence')
        if '/api/' in row.get('path','') and isinstance(row.get('status'),int) and row['status']>=500:
            errors.append('Original API server error')
    if report.get('page_errors'):
        errors.append('Unhandled browser exception')
    return errors

PORTS = {23173, 23174, 23175}


def allowed_request(url):
    try:
        p = urlsplit(url)
        return (p.scheme in {'http','ws'} and p.hostname in {'127.0.0.1','host.docker.internal'}
                and p.port in PORTS and p.username is None and p.password is None)
    except ValueError:
        return False


def request_record(method, url, status):
    p = urlsplit(url)
    return dict(method=method, origin=f'{p.scheme}://{p.hostname}'+(f':{p.port}' if p.port else ''), path=p.path, status=status)


def main():
    from playwright.sync_api import sync_playwright, expect
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cdp', default='http://127.0.0.1:29222')
    parser.add_argument('--host', choices=['127.0.0.1','host.docker.internal'],default='host.docker.internal')
    parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args()
    output=args.out.resolve();output.mkdir(parents=True,exist_ok=False)
    cases=[];network=[];blocked=[];page_errors=[]
    report=dict(scope='original-browser',aip_ported=False,ml='not-executed',
                video_playback='not-executed; provider network blocked',
                cases=cases,network=network,blocked=blocked,page_errors=page_errors,
                auth_values_recorded=False,screenshots=[],complete=False)
    with sync_playwright() as p:
        browser=p.chromium.connect_over_cdp(args.cdp)
        report['browser_version']=browser.version
        for role,port,email in [('user',23173,'user@example.test'),('admin',23174,'admin@example.test')]:
            context=browser.new_context(viewport={'width':1440,'height':960})
            def route(r):
                if allowed_request(r.request.url):
                    r.continue_()
                else:
                    blocked.append(request_record(r.request.method,r.request.url,'blocked'))
                    r.abort()
            context.route('**/*',route)
            def socket_route(ws):
                if allowed_request(ws.url): ws.connect_to_server()
                else:
                    blocked.append(request_record('WS',ws.url,'blocked'))
                    ws.close()
            context.route_web_socket('**/*',socket_route)
            page=context.new_page();page.set_default_timeout(10000)
            page.on('response',lambda r:network.append(request_record(r.request.method,r.url,r.status)))
            page.on('pageerror',lambda e:page_errors.append({'role':role,'kind':type(e).__name__}))
            base=f'http://{args.host}:{port}'
            def screenshot(label):
                name=role+'-'+label+'.png';page.screenshot(path=str(output/name),full_page=True)
                report['screenshots'].append(name)
            def run(label,fn):
                try:
                    fn();screenshot(label);cases.append(dict(role=role,label=label,result='pass'))
                    return True
                except Exception as exc:
                    cases.append(dict(role=role,label=label,result='fail',error_kind=type(exc).__name__,path=urlsplit(page.url).path))
                    screenshot(label+'-failure')
                    return False
            def login():
                page.goto(base+('/auth/1' if role=='user' else '/login'))
                page.wait_for_load_state('networkidle')
                page.locator('input[type=email]').fill(email)
                if role=='user':
                    page.get_by_role('button',name='다음',exact=True).click()
                page.locator('input[type=password]').fill('Synthetic-test-123!')
                page.get_by_role('button',name='로그인',exact=True).click()
                page.wait_for_url(base+('/main' if role=='user' else '/'))
                page.wait_for_load_state('networkidle')
                expect(page.get_by_text('합성사용자',exact=False).first).to_be_visible()
            if not run('login',login):
                context.close();continue
            def visit(path,required):
                page.goto(base+path);page.wait_for_load_state('networkidle')
                expect(page.get_by_text(required,exact=False).first).to_be_visible()
            if role=='user':
                run('main',lambda:visit('/main','오늘의 영상'))
                run('search',lambda:visit('/main?q=합성','검색'))
                run('bookmarks',lambda:visit('/bookmark','나의 즐겨찾기 영상'))
                run('mypage',lambda:visit('/my','합성사용자'))
                run('profile',lambda:visit('/edit','프로필'))
                def watch():
                    page.goto(base+'/watch/00000000-0000-4000-8000-000000000010')
                    page.wait_for_load_state('networkidle')
                    expect(page.get_by_text('합성 영상',exact=True)).to_be_visible()
                    # A blocked player remains an explicit incomplete provider gate.
                    expect(page.get_by_text('영상을 불러올 수 없어요',exact=True)).to_be_visible()
                run('watch-provider-error-state',watch)
            else:
                for label,path,required in [('dashboard','/','대시보드'),('users','/users','user@example.test'),
                                            ('videos','/videos','합성 영상'),('requests','/video-requests','영상 요청'),
                                            ('comments','/comments','댓글 관리')]:
                    run(label,lambda path=path,required=required:visit(path,required))
            context.close()
        browser.close()
    report['complete']=False
    report['planned_cases']=len(PLANNED)
    report['validation_errors']=check_browser_report(report)
    report['partial_browser_run_completed']=not report['validation_errors']
    (output/'browser-result.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps({'cases':len(cases),'failed':sum(c['result']!='pass' for c in cases),
                      'screenshots':len(report['screenshots']),'provider_blocked':len(blocked),
                      'whole_browser_gate_complete':False},ensure_ascii=False))
    return 0 if report['partial_browser_run_completed'] else 1


if __name__=='__main__':
    raise SystemExit(main())
