// The real socket.io-client, configured the way the FaceReview player configures it (src/socket/index.ts of the UI
// 2e7c021): websocket transport only, the token handed over by an `auth` callback, `watch_frame` emitted with an
// acknowledgement callback. Prints one JSON line with what the client observed.
//   node check.mjs <host:port> <good token> <origin>
import { io } from 'socket.io-client';

const [address, goodToken, origin] = process.argv.slice(2);
const url = `http://${address}`;

const options = (token) => ({
  transports: ['websocket'],
  reconnection: false,
  autoConnect: false,
  extraHeaders: { Origin: origin },
  auth: (cb) => cb({ token }),
});

const within = (promise, ms, label) =>
  Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error(`timeout: ${label}`)), ms))]);

const frame = (extra = {}) => ({
  video_view_log_id: '11111111-1111-4111-8111-111111111111',
  youtube_running_time: parseFloat((1.5).toFixed(2)),
  frame_data: 'data:image/jpeg;base64,AAAA',
  duration: 60,
  user_id: 'claimed-by-the-client',
  video_id: '22222222-2222-4222-8222-222222222222',
  ...extra,
});

const emitWithAck = (socket, payload) => new Promise((resolve) => socket.emit('watch_frame', payload, resolve));

const result = {};

// A refused token: the client reports connect_error, never connect.
{
  const socket = io(url, options('nobody'));
  const outcome = within(
    new Promise((resolve) => {
      socket.on('connect', () => resolve({ connected: true }));
      socket.on('connect_error', (err) => resolve({ connected: false, message: err.message, data: err.data ?? null }));
    }),
    5000,
    'refused token',
  );
  socket.connect();
  result.refused = await outcome;
  socket.close();
}

// A good token: connect, one valid frame, one frame with a missing field, then a client-side disconnect.
{
  const socket = io(url, options(goodToken));
  const connected = within(
    new Promise((resolve, reject) => {
      socket.on('connect', () => resolve({ id: socket.id }));
      socket.on('connect_error', (err) => reject(err));
    }),
    5000,
    'connect',
  );
  const closed = new Promise((resolve) => socket.on('disconnect', (reason) => resolve(reason)));
  socket.connect();
  result.connected = await connected;
  result.valid = await within(emitWithAck(socket, frame()), 5000, 'valid frame ack');
  result.missing = await within(emitWithAck(socket, frame({ video_id: undefined })), 5000, 'missing field ack');
  result.unknownEvent = await within(new Promise((resolve) => socket.emit('no_such_event', {}, resolve)), 5000, 'unknown event ack');
  socket.disconnect();
  result.disconnectReason = await within(closed, 5000, 'disconnect');
}

console.log(JSON.stringify(result));
process.exit(0);
