import {test} from 'node:test';
import assert from 'node:assert/strict';
import dgram from 'node:dgram';
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
