import {test} from 'node:test';
import assert from 'node:assert/strict';
import dgram from 'node:dgram';
import net from 'node:net';
import {echoPair} from './network-echo.mjs';

test('network fixture reserves the same TCP and UDP port and echoes a datagram', async () => {
  const {tcp, udp, port} = await echoPair();
  const client = dgram.createSocket('udp4');
  try {
    assert.equal(tcp.address().port, udp.address().port);
    const received = new Promise((resolve, reject) => { client.once('message', resolve); client.once('error', reject); });
    client.send('probe', port, '127.0.0.1');
    assert.equal((await received).toString(), 'probe');
  } finally { client.close(); udp.close(); await new Promise(resolve => tcp.close(resolve)); }
});

test('network fixture retries when the UDP-selected port is already occupied by TCP', async (t) => {
  const occupied = net.createServer();
  await new Promise(resolve => occupied.listen(0, '127.0.0.1', resolve));
  const createSocket = dgram.createSocket;
  let attempts = 0;
  t.mock.method(dgram, 'createSocket', (...args) => {
    const socket = createSocket(...args);
    if (attempts++ === 0) {
      const bind = socket.bind;
      socket.bind = function(port, ...rest) {
        assert.equal(port, 0, 'UDP must choose the port before TCP');
        return bind.call(this, occupied.address().port, ...rest);
      };
    }
    return socket;
  });
  let pair;
  try {
    pair = await echoPair();
    assert.ok(attempts >= 2);
    assert.equal(pair.tcp.address().port, pair.udp.address().port);
    assert.notEqual(pair.port, occupied.address().port);
  } finally {
    pair?.udp.close();
    if (pair) await new Promise(resolve => pair.tcp.close(resolve));
    await new Promise(resolve => occupied.close(resolve));
  }
});
