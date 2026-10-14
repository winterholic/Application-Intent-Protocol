"""Serve pinned ORIGINAL handlers over loopback after the API fixture seeded DB.

No source changes, production endpoints, .env loading or fake ML inference.
"""
import argparse
import hashlib
import json
import os
import sys
from pathlib import Path


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', required=True, type=Path)
    p.add_argument('--inventory', required=True, type=Path)
    args = p.parse_args()
    source = args.source.resolve()
    inventory = json.loads(args.inventory.read_text())
    repo = next(r for r in inventory['repositories'] if r['role']=='backend')
    for file in repo['files']:
        if file['path'].endswith('.py'):
            value = (source/file['path']).read_bytes().replace(b'\r\n',b'\n')
            if hashlib.sha256(value).hexdigest() != file['sha256']:
                raise RuntimeError('Pinned source hash mismatch: '+file['path'])
    if (source/'.env').exists():
        raise RuntimeError('Source must not contain .env')
    os.environ.update(dict(DB_USERNAME='baseline',DB_PASSWORD='synthetic-baseline-only',
        DB_HOST='127.0.0.1',DB_PORT='23306',DB_NAME='facereview_baseline',
        MONGO_HOST='127.0.0.1',MONGO_PORT='27027',MONGO_DB_NAME='facereview_baseline',
        MONGO_USERNAME='',MONGO_PASSWORD='',REDIS_URL='redis://127.0.0.1:26379/0',
        REDIS_HOST='127.0.0.1',REDIS_PORT='26379',REDIS_PASSWORD='',REDIS_DB='0',
        SECRET_KEY='synthetic-baseline-flask-key',JWT_SECRET_KEY='synthetic-baseline-jwt-key',
        SMTP_USERNAME='',SMTP_PASSWORD='',YOUTUBE_API_KEY=''))
    sys.path.insert(0,str(source)); os.chdir(source)
    from app import create_app
    app = create_app('production',preload_emotion_model=False)
    from common.extensions import db
    from app.models.user import User
    with app.app_context():
        if db.session.get(User,'00000000-0000-4000-8000-000000000001') is None:
            raise RuntimeError('Missing owned baseline seed; run API baseline first')
    app.run(host='127.0.0.1',port=23175,debug=False,use_reloader=False)


if __name__=='__main__':
    main()
