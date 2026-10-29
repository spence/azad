"""Measure an owned Codex prompt through a private PTY, without a model request."""
import codecs
import uuid
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import sys
import subprocess
import termios
import time

if len(sys.argv) > 1 and sys.argv[1] == '--writer':
    fd, rate, text = int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    started = time.monotonic()
    posts = []
    due = started
    for ch in text:
        if rate:
            time.sleep(max(0, due - time.monotonic()))
        assert os.write(fd, ch.encode()) == len(ch.encode())
        posted = time.monotonic()
        posts.append((posted - started) * 1000)
        due = posted + (1 / rate if rate else 0)
    print(json.dumps({'started': started, 'posts_ms': posts, 'emission_ms': (time.monotonic() - started) * 1000}))
    sys.exit(0)

RUN_ID = str(uuid.uuid4())
ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'python-libs'))
import pyte

samples = [item['text'] for item in json.loads((ROOT / 'reported-payloads.json').read_text())]
assert all(sample and sample == sample.lstrip() and len(sample) < 900 and all(c.isprintable() for c in sample) for sample in samples)
screen = pyte.Screen(186, 56)
stream = pyte.Stream(screen)
decoder = codecs.getincrementaldecoder('utf-8')('replace')
pending = b''
raw = []
frames = []
started = None
visible = ''
exited = False


def read_for(seconds):
    global pending, visible, exited
    if not select.select([master], [], [], max(0, seconds))[0]:
        return
    try:
        data = os.read(master, 65536)
    except OSError as error:
        if error.errno != errno.EIO:
            raise
        data = b''
    if not data:
        exited = True
        return
    now = time.monotonic()
    raw.append({'at_ns': time.monotonic_ns(), 'hex': data.hex()})
    pending += data
    if b'\x1b[6n' in data:
        os.write(master, b'\x1b[1;1R')
    end = b'\x1b[?2026l'
    while end in pending:
        frame, pending = pending.split(end, 1)
        stream.feed(decoder.decode(frame + end))
        rows = screen.display
        candidates = [i for i, row in enumerate(rows) if row.lstrip().startswith('›')]
        current = ''
        if candidates:
            index = candidates[-1]
            parts = [rows[index].split('›', 1)[1].strip()]
            for row in rows[index + 1:]:
                if not row.strip():
                    break
                parts.append(row.strip())
            current = ' '.join(parts)
            if current.startswith('Ask Codex'):
                current = ''
        if current != visible:
            visible = current
            if started is not None:
                frames.append({'ms': round((now - started) * 1000, 3), 'text': current})


def until(predicate, timeout):
    deadline = time.monotonic() + timeout
    while not predicate():
        if exited or time.monotonic() > deadline:
            raise RuntimeError(f'PTY deadline; last prompt={visible!r}')
        read_for(min(0.02, deadline - time.monotonic()))


def emit(data):
    assert os.write(master, data) == len(data)


client = 'private' if '--private-client' in sys.argv else 'gateway'
command = (['/Users/spencer/.codex/packages/standalone/releases/0.163.0-aarch64-apple-darwin/bin/codex', '--no-daemon']
           if client == 'private' else ['lag', 'codex', 'tui'])
command += ['--no-alt-screen', '-c', 'check_for_update_on_startup=false',
            '-c', 'mcp_servers={}', '-c', f'projects."{ROOT}".trust_level="trusted"']
if '--disable-burst' in sys.argv:
    command += ['-c', 'tui.disable_paste_burst=true']
capture_prefix = 'codex-pty-' + client + ('-burst-off' if '--disable-burst' in sys.argv else '') + '-186x56-' + RUN_ID
raw_path = ROOT / (capture_prefix + '-raw.jsonl')
exit_path = ROOT / (capture_prefix + '-exit.json')
pid, master = pty.fork()
if pid == 0:
    try:
        os.chdir(ROOT)
        os.environ['TERM'] = 'xterm-256color'
        os.execvp(command[0], command)
    finally:
        os._exit(127)

try:
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 56, 186, 0, 0))
    os.set_blocking(master, False)
    until(lambda: any('Ask Codex' in row for row in screen.display), 20)
    emit(b'AZAD_WARMUP')
    until(lambda: visible == 'AZAD_WARMUP', 15)
    emit(b'\x15')
    until(lambda: visible == '', 5)
    cases = [('raw', 0, samples[0]), ('single_write', 0, samples[0]), ('bracketed_paste', 0, samples[0]), *[('raw', 0, sample) for sample in samples[1:]], ('raw', 320, samples[0]), ('raw', 80, samples[0])]
    for method, rate, sample in cases:
        expected = sample.rstrip()
        frames = []
        started = time.monotonic()
        posts = []
        if method in ('single_write', 'bracketed_paste'):
            payload = sample.encode()
            if method == 'bracketed_paste':
                payload = b'\x1b[200~' + payload + b'\x1b[201~'
            emit(payload)
            posts.append((time.monotonic() - started) * 1000)
        else:
            writer = subprocess.Popen([sys.executable, __file__, '--writer', str(master), str(rate), sample],
                                      pass_fds=(master,), stdout=subprocess.PIPE, text=True)
            try:
                until(lambda: writer.poll() is not None, 10)
                metadata = json.loads(writer.communicate(timeout=1)[0])
                if writer.returncode:
                    raise RuntimeError('The owned PTY writer failed.')
                offset = (metadata['started'] - started) * 1000
                posts = [item + offset for item in metadata['posts_ms']]
                emitted_ms = metadata['emission_ms']
            finally:
                if writer.poll() is None:
                    writer.kill()
                    writer.wait(timeout=2)
        if method in ('single_write', 'bracketed_paste'):
            emitted_ms = (time.monotonic() - started) * 1000
        deadline = time.monotonic() + 5
        while visible != expected and not exited and time.monotonic() < deadline:
            read_for(min(0.02, deadline - time.monotonic()))
        matches = visible == expected
        changed = [frame for frame in frames if frame['text']]
        row = {'run_id': RUN_ID, 'raw_file': raw_path.name, 'exit_file': exit_path.name, 'expected_visible_text': expected, 'client': client, 'disable_burst': '--disable-burst' in sys.argv, 'method': method, 'rate': rate, 'chars': len(sample),
               'emission_ms': round(emitted_ms, 3), 'posts_ms': posts,
               'first_observed_prompt_ms': changed[0]['ms'] if changed else None, 'last_observed_prompt_ms': changed[-1]['ms'] if changed else None,
               'full_observed_prompt_ms': changed[-1]['ms'] if changed and matches else None, 'final_observed_text': visible,
               'columns': 186, 'rows': 56, 'observation': 'PTY output only; wrapped display words are joined with spaces; frames within one read share its receipt timestamp',
               'changed_frames': len(changed), 'visible_words_and_punctuation_match': matches,
               'frames': frames}
        print(json.dumps(row), flush=True)
        started = None
        emit(b'\x1b')
        read_for(0.05)
        emit(b'\x15')
        until(lambda: visible == '', 5)
finally:
    try:
        try:
            os.write(master, b'\x15\x04')
        except OSError:
            pass
        deadline = time.monotonic() + 5
        ended = False
        forced = False
        client_exit = None
        while time.monotonic() < deadline:
            child, status = os.waitpid(pid, os.WNOHANG)
            if child:
                client_exit = os.waitstatus_to_exitcode(status)
                ended = True
                break
            read_for(0.02)
        if not ended:
            os.close(master)
            master = None
            # this exact child is the private PTY client, not its shared backend.
            forced = True
            os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                child, status = os.waitpid(pid, os.WNOHANG)
                if child:
                    client_exit = os.waitstatus_to_exitcode(status)
                    ended = True
                    break
                time.sleep(0.02)
            if not ended:
                os.kill(pid, signal.SIGKILL)
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    child, status = os.waitpid(pid, os.WNOHANG)
                    if child:
                        client_exit = os.waitstatus_to_exitcode(status)
                        ended = True
                        break
                    time.sleep(0.02)
                if not ended:
                    raise RuntimeError(f'Owned PTY child {pid} did not exit.')
    finally:
        if master is not None:
            os.close(master)
        exit_path.write_text(json.dumps({'run_id': RUN_ID, 'pid': pid, 'ended': ended, 'exit_code': client_exit, 'forced': forced}) + '\n')
        raw_path.write_text('\n'.join(json.dumps(item) for item in raw) + '\n')
