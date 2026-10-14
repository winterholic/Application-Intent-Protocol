import unittest

from facereview_inventory import python_entries, frontend_entries


class InventoryTests(unittest.TestCase):
    def test_blueprint_prefix_methods_auth_and_development_scope(self):
        rows = python_entries('app/routes/auth.py', '''
bp = Blueprint('auth', __name__, url_prefix='/v2/auth')
test_bp = Blueprint('auth_test', __name__, url_prefix='/test')
@bp.route('/me', methods=['GET', 'PATCH'])
@login_required
def me():
    return AuthService.me()
@test_bp.route('/token', methods=['POST'])
def token(): pass
''')
        self.assertEqual([r['endpoint'] for r in rows], ['GET /v2/auth/me', 'PATCH /v2/auth/me', 'POST /test/token'])
        self.assertEqual(rows[0]['policy'], ['login_required'])
        self.assertEqual(rows[0]['calls'], ['AuthService.me'])
        self.assertEqual(rows[-1]['scope'], 'development-only')

    def test_dynamic_route_fails_instead_of_disappearing(self):
        with self.assertRaisesRegex(ValueError, 'literal'):
            python_entries('app/routes/new.py', '@bp.route(PATH)\ndef new(): pass')

    def test_socket_job_and_direct_health_are_entrypoints(self):
        rows = python_entries('app/__init__.py', '''
@socketio.on('watch_frame')
def frame(): pass
@celery_app.task(name='rebuild')
def rebuild(): pass
@app.route('/health')
def health(): pass
''')
        self.assertEqual([r['kind'] for r in rows], ['socket', 'job', 'http'])
        self.assertEqual(rows[-1]['endpoint'], 'GET /health')

    def test_frontend_calls_include_generics_multiline_and_dynamic_urls(self):
        text = '''
api.get<Response>(`/users/${id}`);
api.post(
  '/login', body);
socket.emit('watch_frame', frame);
<Route path="/watch/:id" element={<Watch />} />
'''
        rows = frontend_entries('src/App.tsx', text)
        self.assertEqual({r['endpoint'] for r in rows}, {'GET /users/${id}', 'POST /login', 'emit watch_frame', '/watch/:id'})

    def test_unparsed_frontend_http_call_fails_closed(self):
        with self.assertRaisesRegex(ValueError, 'Unparsed'):
            frontend_entries('src/api.ts', 'api.get(urlFromUser)')

    def test_function_local_url_constants_and_layout_children(self):
        rows = frontend_entries('src/api.ts', '''
export const list = () => { const url = '/one'; return api.get(url); };
export const add = () => { const url = '/two'; return api.post(url); };
<Route element={<Layout />}><Route path="/child" /></Route>
''')
        self.assertEqual([r['endpoint'] for r in rows], ['GET /one', 'POST /two', '/child'])

    def test_inline_response_types_do_not_hide_calls(self):
        rows = frontend_entries('src/api.ts', "api.post<{ access_token: string; other: string; }>('/reissue');")
        self.assertEqual(rows[0]['endpoint'], 'POST /reissue')

    def test_new_transport_idiom_requires_explicit_extractor_support(self):
        for source in ("fetch('/new')", "api.request({url: '/new'})", "api.head('/new')"):
            with self.subTest(source=source), self.assertRaisesRegex(ValueError, 'Unsupported'):
                frontend_entries('src/new.ts', source)


if __name__ == '__main__':
    unittest.main()
