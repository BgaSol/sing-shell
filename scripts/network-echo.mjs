import net from 'node:net';
import dgram from 'node:dgram';

// Windows TCP allocation can repeatedly select a UDP-excluded range. Let UDP choose first.
export async function echoPair() {
  for (let attempt = 0; attempt < 10; attempt++) {
    const tcp = net.createServer(socket => socket.pipe(socket));
    const udp = dgram.createSocket('udp4');
    udp.on('message', (message, remote) => udp.send(message, remote.port, remote.address));
    try {
      await new Promise((resolve, reject) => { udp.once('error', reject); udp.bind(0, '127.0.0.1', resolve); });
      const port = udp.address().port;
      await new Promise((resolve, reject) => { tcp.once('error', reject); tcp.listen(port, '127.0.0.1', resolve); });
      return {tcp, udp, port};
    } catch (error) {
      if (tcp.listening) await new Promise(resolve => tcp.close(resolve));
      try { udp.close(); } catch {}
      if (!['EACCES', 'EADDRINUSE'].includes(error.code) || attempt === 9) throw error;
    }
  }
}
