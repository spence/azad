"""Check the producer publication gap against the owned guest Jump VNC connection."""
import json
import os
from pathlib import Path
import subprocess
import time
import uuid
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parent
ENV = dict(os.environ, TART_HOME=str(ROOT / 'tart'), TART_NO_AUTO_PRUNE='1')
PREFIX = ['tart', 'exec', 'azad-insertion']
RUN_ID = str(uuid.uuid4())
STARTED = datetime.now(timezone.utc).isoformat()

def run(*args):
    result = subprocess.run(PREFIX + list(args), env=ENV, text=True,
                            capture_output=True, timeout=15)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result.stdout + result.stderr

def records():
    rows = [json.loads(line) for line in run('/bin/cat', '/Users/admin/rfb-publication-gap.jsonl').splitlines()]
    if not any(row['kind'] == 'ready' for row in rows) or any(row['kind'] == 'end' for row in rows):
        raise RuntimeError('the guest receiver is not admitted and live')
    if any(row['commit_delay_ms'] != 0 for row in rows if row['kind'] == 'clipboard_received'):
        raise RuntimeError('the receiver must not delay clipboard commit')
    return rows

def pastes():
    return [row for row in records() if row['kind'] == 'paste']

def wait_for_paste(previous):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        rows = pastes()
        if len(rows) == previous + 1:
            return rows[-1]
        if len(rows) > previous + 1:
            raise RuntimeError('unexpected additional paste')
        time.sleep(0.05)
    raise RuntimeError('the guest receiver did not receive a paste')

for mode in ['clear', 'declare']:
    seed = 'AZAD_PUBLICATION_SEED_' + mode.upper()
    previous = len(pastes())
    run('/Users/admin/azad-insertion-probe', '--allow-vm-jump-input', seed)
    if wait_for_paste(previous)['clipboard'] != seed:
        raise RuntimeError('the positive seed did not reach the receiver')
    text = 'AZAD_PUBLICATION_NEW_' + mode.upper()
    publication = run('/Users/admin/clipboard-publication-gap', mode, text)
    previous = len(pastes())
    producer = run('/Users/admin/azad-insertion-probe', '--allow-vm-jump-retry', text)
    received = wait_for_paste(previous)
    matches = received['clipboard'] == text
    print(json.dumps({'run_id': RUN_ID, 'started_utc': STARTED,
                      'receiver_paste_time_ns': received['time_ns'], 'receiver_commit_delay_ms': 0,
                      'mode': mode, 'expected': text, 'publication': publication,
                      'producer': producer, 'receiver_clipboard': received['clipboard'],
                      'receiver_matches': matches}), flush=True)
    if not matches:
        raise SystemExit(2)
