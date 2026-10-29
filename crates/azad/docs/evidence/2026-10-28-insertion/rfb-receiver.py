"""Loopback-only RFB receiver for native input from the owned VM, never the host desktop."""
import asyncio
import json
import hashlib
import signal
import sys
import struct
import time

OLD = "0050's null wrapper: the inline oneof fixes this, with less clutter."

def record(kind, **fields):
    print(json.dumps(dict(kind=kind, time_ns=time.monotonic_ns(), **fields)), flush=True)

async def client(reader, writer):
    async def read(size):
        if not 0 <= size <= 1_048_576:
            raise ValueError(f"invalid payload size {size}")
        return await asyncio.wait_for(reader.readexactly(size), 120)

    async def send(data):
        writer.write(data)
        await writer.drain()

    clipboard = OLD
    modifiers = set()
    field = ""
    keys = 0
    generation = 0
    commits = set()
    delay_ms = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    if not 0 <= delay_ms <= 2000:
        raise ValueError('clipboard commit delay must be between 0 and 2000 ms')

    def snapshot():
        record('snapshot', field=field, chars=len(field),
               sha256=hashlib.sha256(field.encode()).hexdigest(), clipboard=clipboard)

    asyncio.get_running_loop().add_signal_handler(signal.SIGUSR1, snapshot)

    async def commit(text, number):
        nonlocal clipboard
        if delay_ms:
            await asyncio.sleep(delay_ms / 1000)
        clipboard = text
        record('clipboard_committed', generation=number, text=text)

    try:
        await send(b'RFB 003.008\n')
        version = await read(12)
        record('version', version=version.decode('ascii'))
        if version == b'RFB 003.003\n':
            await send(struct.pack('>I', 1))
        elif version in (b'RFB 003.007\n', b'RFB 003.008\n'):
            await send(b'\x01\x01')
            if await read(1) != b'\x01':
                raise ValueError('the receiver permits only loopback no-auth security')
            if version == b'RFB 003.008\n':
                await send(struct.pack('>I', 0))
        else:
            raise ValueError('unsupported protocol version')
        await read(1)
        name = b'Azad isolated clipboard receiver'
        pixels = struct.pack('>BBBBHHHBBB3x', 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
        await send(struct.pack('>HH', 640, 480) + pixels + struct.pack('>I', len(name)) + name)
        old = OLD.encode('latin-1')
        await send(b'\x03\x00\x00\x00' + struct.pack('>I', len(old)) + old)
        record('ready', clipboard=clipboard)
        initial = True
        while True:
            kind = (await reader.readexactly(1))[0]
            if kind == 0:
                await read(3)
                pixels = await read(16)
                record('pixel_format', bits=pixels[0])
            elif kind == 2:
                data = await read(3)
                count = struct.unpack('>H', data[1:])[0]
                encodings = struct.unpack('>' + 'i' * count, await read(4 * count))
                record('encodings', values=encodings)
            elif kind == 3:
                await read(9)
                if initial:
                    initial = False
                    if pixels[0] not in (8, 16, 32):
                        raise ValueError('unsupported pixel size')
                    body = bytes(640 * 480 * (pixels[0] // 8))
                    await send(struct.pack('>BBHHHHHi', 0, 0, 1, 0, 0, 640, 480, 0) + body)
            elif kind == 4:
                data = await read(7)
                down, key = data[0], struct.unpack('>I', data[3:])[0]
                if key in (0xffe3, 0xffe4, 0xffe7, 0xffe8, 0xffeb, 0xffec):
                    if down: modifiers.add(key)
                    else: modifiers.discard(key)
                if down and modifiers and key in (ord('v'), ord('V')):
                    field += clipboard
                    record('paste', clipboard=clipboard, field=field, modifiers=sorted(modifiers))
                elif down and modifiers and key in (ord('a'), ord('A')):
                    field = ''
                elif down and not modifiers:
                    code = key & 0xffffff if key & 0xff000000 == 0x01000000 else key
                    if code <= 0x10ffff and (code < 0xff00 or key & 0xff000000 == 0x01000000):
                        field += chr(code)
                keys += 1
                if key == 0xff0d and down and not modifiers:
                    field += '\n'
                if keys <= 64 or keys % 1000 == 0:
                    record('key', key=key, down=bool(down), field_chars=len(field), events=keys)
            elif kind == 5:
                await read(5)
            elif kind == 6:
                data = await read(7)
                length = struct.unpack('>I', data[3:])[0]
                text = (await read(length)).decode('latin-1')
                generation += 1
                record('clipboard_received', generation=generation, text=text, commit_delay_ms=delay_ms)
                task = asyncio.create_task(commit(text, generation))
                commits.add(task)
                task.add_done_callback(commits.discard)
            else:
                raise ValueError(f'unsupported client message {kind}')
    except Exception as error:
        record('end', error=str(error))
    finally:
        for task in commits:
            task.cancel()
        await asyncio.gather(*commits, return_exceptions=True)
        snapshot()
        writer.close()
        await writer.wait_closed()

async def main():
    server = await asyncio.start_server(client, '127.0.0.1', 15900)
    record('listening', address='127.0.0.1:15900')
    async with server:
        await server.serve_forever()

try:
    asyncio.run(main())
except KeyboardInterrupt:
    pass
