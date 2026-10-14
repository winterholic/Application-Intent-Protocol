"""Run the pinned ORIGINAL backend against disposable MySQL/Mongo/Redis.

This is an API baseline, never an AIP port or a browser/ML baseline.
No .env is read. Output contains contracts and status codes, never JWTs.
"""
import argparse
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import sys
from datetime import datetime


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--inventory', type=Path, required=True)
    args = parser.parse_args()
    args.source = args.source.resolve()
    args.output = args.output.resolve()
    inventory = json.loads(args.inventory.read_text())
    backend = next(r for r in inventory['repositories'] if r['role'] == 'backend')
    for file in backend['files']:
        if not file['path'].endswith('.py'):
            continue
        raw = (args.source / file['path']).read_bytes().replace(b'\r\n', b'\n')
        if hashlib.sha256(raw).hexdigest() != file['sha256']:
            raise RuntimeError('Source differs from pinned revision: '+file['path'])
    if (args.source / '.env').exists():
        raise RuntimeError('Baseline source must not contain .env')
    os.environ.update(dict(
        DB_USERNAME='baseline', DB_PASSWORD='synthetic-baseline-only',
        DB_HOST='127.0.0.1', DB_PORT='23306', DB_NAME='facereview_baseline',
        MONGO_HOST='127.0.0.1', MONGO_PORT='27027', MONGO_DB_NAME='facereview_baseline',
        MONGO_USERNAME='', MONGO_PASSWORD='',
        REDIS_URL='redis://127.0.0.1:26379/0', REDIS_HOST='127.0.0.1', REDIS_PORT='26379',
        REDIS_PASSWORD='', REDIS_DB='0',
        SECRET_KEY='synthetic-baseline-flask-key', JWT_SECRET_KEY='synthetic-baseline-jwt-key',
        SMTP_USERNAME='', SMTP_PASSWORD='', YOUTUBE_API_KEY='',
    ))
    sys.path.insert(0, str(args.source.resolve()))
    os.chdir(args.source)
    from app import create_app
    # Boot exactly as run.py does before importing model packages; early model
    # imports can bind service globals before the factory initializes Redis.
    app = create_app('production', preload_emotion_model=False)
    from common.extensions import db
    from common import extensions
    from app.models.user import User
    from app.models.video import Video
    from app.models.comment import Comment
    from app.models.video_like import VideoLike
    from app.models.video_bookmark import VideoBookmark
    from werkzeug.security import generate_password_hash
    from sqlalchemy import text

    password = 'Synthetic-test-123!'
    video_id = '00000000-0000-4000-8000-000000000010'
    user_id = '00000000-0000-4000-8000-000000000001'
    admin_id = '00000000-0000-4000-8000-000000000002'
    with app.app_context():
        db.create_all()
        if db.session.query(User).count():
            raise RuntimeError('Expected fresh disposable database; reset this compose project first')
        for ident, email, role in [(user_id,'user@example.test','GENERAL'), (admin_id,'admin@example.test','SUPER_ADMIN')]:
            db.session.add(User(user_id=ident, email=email,
                                password=generate_password_hash(password),
                                name='합성사용자', role=role, created_at=datetime(2026,1,1)))
        db.session.add(Video(video_id=video_id, youtube_url='dQw4w9WgXcQ', title='합성 영상',
                              channel_name='합성 채널', category='music', duration=120,
                              created_at=datetime(2026,1,1)))
        db.session.commit()
        versions = dict(mysql=db.session.execute(text('SELECT VERSION()')).scalar(),
                        mongo=extensions.mongo_client.server_info()['version'],
                        redis=extensions.redis_client.info()['redis_version'])
    client = app.test_client()
    admin_client = app.test_client()
    observations = []
    anomalies = []

    def call(label, method, path, expected, body=None, token=None, api_client=None):
        headers = {'Authorization': 'Bearer '+token} if token else {}
        response = (api_client or client).open(path, method=method, json=body, headers=headers)
        data = response.get_json()
        observations.append(dict(label=label, method=method, path=path,
                                 status=response.status_code, expected=expected,
                                 response_keys=sorted(data) if isinstance(data,dict) else [],
                                 code=data.get('code') if isinstance(data,dict) else None))
        if response.status_code != expected:
            # Only synthetic input; auth payloads are deliberately excluded.
            raise AssertionError(f'{label}: {response.status_code}, expected {expected}; keys={observations[-1]["response_keys"]}')
        return data

    try:
        call('health','GET','/health',200)
        user = call('login-user','POST','/api/v2/auth/login',200,dict(email='user@example.test',password=password))['access_token']
        from app.routes.auth import REFRESH_TOKEN_COOKIE_NAME
        user_refresh = client.get_cookie(REFRESH_TOKEN_COOKIE_NAME)
        assert user_refresh is not None
        admin = call('login-admin','POST','/api/v2/auth/login',200,dict(email='admin@example.test',password=password),api_client=admin_client)['access_token']
        call('me','GET','/api/v2/auth/me',200,token=user)
        call('guest-me-denied','GET','/api/v2/auth/me',401)
        call('general-admin-denied','GET','/api/v2/admin/users',403,token=user)
        call('admin-users','GET','/api/v2/admin/users',200,token=admin)
        call('tutorial','PATCH','/api/v2/auth/tutorial',200,token=user)
        call('guest-video-detail','GET',f'/api/v2/watch/video?video_id={video_id}',200)
        call('guest-comment-denied','POST','/api/v2/watch/comments',401,dict(video_id=video_id,content='거절해야 함'))
        comment = call('create-comment','POST','/api/v2/watch/comments',200,dict(video_id=video_id,content='합성 댓글'),user)['comment_id']
        if comment is None:
            # Keep the original response defect visible, then use the DB oracle
            # to exercise subsequent handlers without changing source code.
            with app.app_context():
                comment = db.session.query(Comment).filter_by(user_id=user_id, video_id=video_id).one().comment_id
            anomalies.append(dict(case='create-comment', observed='HTTP 200 comment_id=null; committed row has UUID',
                                  source='app/services/watch_service.py:add_comment',
                                  cause='DTO captures default-generated ID before session flush',
                                  followup='Subsequent API cases use actual DB UUID; original response is not repaired'))
        call('edit-comment','PATCH','/api/v2/watch/comments',200,dict(comment_id=comment,content='수정 댓글'),user)
        call('list-comments','GET',f'/api/v2/watch/comments?video_id={video_id}',200,token=user)
        call('delete-comment','DELETE',f'/api/v2/watch/comments?comment_id={comment}',200,token=user)
        with app.app_context():
            saved = db.session.get(Comment,comment)
            assert saved is not None and saved.is_deleted, 'Original comment delete must be soft delete'
        for i in range(3):
            call(f'like-toggle-{i+1}','POST','/api/v2/watch/like',200,dict(video_id=video_id),user)
            call(f'bookmark-toggle-{i+1}','POST','/api/v2/home/bookmark',200,dict(video_id=video_id),user)
            with app.app_context():
                for model in (VideoLike,VideoBookmark):
                    assert db.session.query(model).filter_by(user_id=user_id,video_id=video_id).count() == (1 if i%2==0 else 0)
        call('logout','POST','/api/v2/auth/logout',200,token=user)
        assert client.get_cookie(REFRESH_TOKEN_COOKIE_NAME) is None
        assert extensions.redis_client.exists('facereview:blacklist:'+user_refresh.value)
        # The source route revokes only the refresh cookie, not access tokens.
        call('source-access-token-lives-until-expiry','GET','/api/v2/auth/me',200,token=user)
        client.set_cookie(REFRESH_TOKEN_COOKIE_NAME,user_refresh.value)
        replay = call('source-revoked-refresh-replay','POST','/api/v2/auth/reissue',200)
        assert replay.get('access_token')
        anomalies.append(dict(case='source-revoked-refresh-replay',
                              observed='Refresh token exists in Redis blacklist but reissue returns 200 and new access token',
                              source='app/services/auth_service.py:reissue',
                              cause='Reissue verifies JWT and active user but does not consult refresh blacklist',
                              followup='Security difference: port must reject replay; preserve source observation, not vulnerability'))
    finally:
        expected = {(r['endpoint'].split(' ',1)[0],r['endpoint'].split(' ',1)[1])
                    for r in inventory['entries'] if r['kind']=='http' and r['scope']=='product'}
        actual = {(m,rule.rule) for rule in app.url_map.iter_rules()
                  for m in rule.methods-{'HEAD','OPTIONS'} if rule.endpoint != 'static'}
        missing = sorted(expected-actual)
        extra = sorted(actual-expected)
        result = dict(source_sha=backend['sha'], runtime=versions,
                      transport='Flask test_client -> ORIGINAL handlers -> real MySQL/Mongo/Redis',
                      ml='not executed; preload_emotion_model=False; no fake inference',
                      browser='not executed', aip_ported=False, observations=observations,
                      original_anomalies=anomalies,
                      missing_registered_routes=missing, additional_framework_routes=extra,
                      expected_product_http=len(expected),
                      packages=sorted(f'{d.metadata["Name"]}=={d.version}' for d in importlib.metadata.distributions()),
                      seed=dict(user_id=user_id, admin_id=admin_id, video_id=video_id,
                                description='two synthetic roles, one fixed video; no production data'))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n')
        print(json.dumps(dict(cases=len(observations), oracle_mismatches=sum(r['status']!=r['expected'] for r in observations),
                              original_anomalies=len(anomalies), missing_registered_routes=missing, aip_ported=False)))
    if missing:
        raise AssertionError('Inventory contains unregistered production routes')


if __name__ == '__main__':
    main()
