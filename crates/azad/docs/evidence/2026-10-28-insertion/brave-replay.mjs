import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const targets = await (await fetch('http://127.0.0.1:19322/json/list')).json();
const target = targets.find(target => target.type === 'page');
if (!target) throw new Error('The isolated browser has no page target.');
const socket = new WebSocket(target.webSocketDebuggerUrl.replace(':9222/', ':19322/'));
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
let nextId = 0;
const pending = new Map();
socket.addEventListener('message', event => {
  const message = JSON.parse(event.data);
  const item = pending.get(message.id);
  if (!item) return;
  pending.delete(message.id);
  if (message.error) item.reject(new Error(JSON.stringify(message.error)));
  else item.resolve(message.result);
});
function send(method, params = {}) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const deadline = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP deadline: ${method}`));
    }, 15000);
    pending.set(id, {
      resolve(value) { clearTimeout(deadline); resolve(value); },
      reject(error) { clearTimeout(deadline); reject(error); }
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
}
function quote(value) { return "'" + value.replaceAll("'", "'\\''") + "'"; }
function observeInput() {
  return evaluate(`new Promise(resolve => {
    const field = document.querySelector('textarea[name=q],input[name=q]');
    let finished = false;
    const finish = reason => {
      if (finished) return;
      finished = true;
      clearTimeout(deadline);
      field.removeEventListener('input', onInput);
      resolve({ reason, value: field.value });
    };
    const onInput = () => finish('input');
    const deadline = setTimeout(() => finish('deadline'), 3500);
    field.addEventListener('input', onInput);
  })`);
}

try {
  await send('Page.enable');
  await send('Page.navigate', { url: 'https://www.google.com/' });
  await evaluate(`new Promise(resolve => {
    if (document.readyState === 'complete') resolve(true);
    else addEventListener('load', () => resolve(true), { once: true });
  })`);
  console.log(JSON.stringify({ page: await evaluate('location.href'), title: await evaluate('document.title') }));
  await send('Page.bringToFront');
  const cases = [
    { text: 'Does it require a special language server plugin or does it translate all of its types into typescript and typescript will do the work?', stallMs: 0 },
    { text: 'These are typically difficult things to do or impossible to do with typescript, but it seems like archetype supports this.', stallMs: 0 },
    ...[50, 250, 1000].map(stallMs => ({ stallMs })),
    { stallMs: 0, blockVOnce: true }
  ];
  for (const { text, stallMs, blockVOnce = false } of cases) {
    const expected = text ?? `AZAD_REPLAY_${nextId}_${stallMs}_new_transcription`;
    const focused = await evaluate(`(() => {
      const field = document.querySelector('textarea[name=q],input[name=q]');
      if (!field) throw new Error('Google search field is absent.');
      field.value = '';
      field.focus();
      window.azadEvents = [];
      window.azadStallMs = ${stallMs};
      window.azadBlockVOnce = ${blockVOnce};
      if (!window.azadListenersInstalled) {
        window.azadListenersInstalled = true;
        for (const name of ['keydown', 'keyup', 'paste', 'beforeinput', 'input']) {
          document.addEventListener(name, event => {
            window.azadEvents.push({ type: name, key: event.key, meta: event.metaKey,
              time: performance.now(), pasted: event.clipboardData?.getData('text/plain') });
            if (name === 'keydown' && event.metaKey && event.key.toLowerCase() === 'v') {
              if (window.azadBlockVOnce) {
                window.azadBlockVOnce = false;
                event.preventDefault();
                window.azadEvents.at(-1).blockedByFixture = true;
                return;
              }
              const end = performance.now() + window.azadStallMs;
              while (performance.now() < end) {}
            }
          }, true);
        }
      }
      return document.activeElement === field;
    })()`);
    if (!focused) throw new Error('The Google search field did not acquire focus.');
    const received = observeInput();
    const probe = await execute('sshpass', ['-e', 'ssh', '-o', 'StrictHostKeyChecking=yes',
      '-o', 'UserKnownHostsFile=/Users/spencer/projects/azad/insertion-20261028/known_hosts',
      'admin@192.168.64.14', `/Users/admin/azad-insertion-probe --allow-vm-input ${quote(expected)}`],
      { env: { ...process.env, SSHPASS: 'admin' }, timeout: 10000 });
    const outcome = await received;
    const finalTrace = await evaluate(`({ value: document.querySelector('textarea[name=q],input[name=q]').value,
      events: window.azadEvents })`);
    const matches = finalTrace.value === expected;
    console.log(JSON.stringify({ expected, stallMs, blockVOnce, producer: probe.stdout.trim(),
      handoff: probe.stderr.trim().split('\n').filter(line => line.startsWith('AZAD_PASTE_HANDOFF')),
      matches, reason: outcome.reason, firstInputValue: outcome.value, ...finalTrace }));
    if (matches === blockVOnce) throw new Error('The recipient result does not match this scenario.');
    if (blockVOnce) {
      if (!finalTrace.events.some(event => event.blockedByFixture)) throw new Error('The negative control did not block a shortcut.');

    }
  }
  console.log(JSON.stringify({ scenarios: cases.length, summary: 'ok' }));
} finally {
  socket.close();
}
