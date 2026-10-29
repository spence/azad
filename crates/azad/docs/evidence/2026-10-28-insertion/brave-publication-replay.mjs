import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';

const execute = promisify(execFile);
const env = { ...process.env, SSHPASS: 'admin' };
const ssh = ['-e', 'ssh', '-o', 'StrictHostKeyChecking=yes', '-o',
  'UserKnownHostsFile=/Users/spencer/projects/azad/insertion-20261028/known_hosts',
  '-o', 'ConnectTimeout=5', 'admin@192.168.64.14'];
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const guest = command => execute('sshpass', [...ssh, command], { env, timeout: 15000 });
const targets = await (await fetch('http://127.0.0.1:19322/json/list')).json();
const target = targets.find(item => item.type === 'page' && item.url.startsWith('https://www.google.com/'));
if (!target) throw new Error('The owned browser has no Google target.');
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
  clearTimeout(item.deadline);
  message.error ? item.reject(new Error(JSON.stringify(message.error))) : item.resolve(message.result);
});
function send(method, params = {}) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const deadline = setTimeout(() => { pending.delete(id); reject(new Error(`CDP deadline: ${method}`)); }, 15000);
    pending.set(id, { resolve, reject, deadline });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
}
async function focus() {
  await guest("open -a 'Brave Browser'");
  await send('Page.bringToFront');
  if (!await evaluate(`new Promise(resolve => {
    const field = document.querySelector('textarea[name=q],input[name=q]');
    if (!field) throw new Error('The Google field is absent.');
    let finished = false;
    const finish = value => {
      if (finished) return;
      finished = true;
      clearTimeout(deadline);
      window.removeEventListener('focus', check, true);
      resolve(value);
    };
    const check = () => {
      if (document.activeElement !== field) field.focus();
      if (document.hasFocus() && document.activeElement === field) finish(true);
    };
    const deadline = setTimeout(() => finish(false), 3500);
    window.addEventListener('focus', check, true);
    check();
  })`)) throw new Error('The Google field did not acquire focus.');
}
async function reset() {
  await evaluate(`(() => {
    const field = document.querySelector('textarea[name=q],input[name=q]');
    field.value = '';
    window.azadEvents = [];
    if (!window.azadListenersInstalled) {
      window.azadListenersInstalled = true;
      for (const name of ['keydown', 'keyup', 'paste', 'beforeinput', 'input']) {
        document.addEventListener(name, event => window.azadEvents.push({
          type: name, key: event.key, meta: event.metaKey, time: performance.now(),
          pasted: event.clipboardData?.getData('text/plain')
        }), true);
      }
    }
  })()`);
  await focus();
}
async function paste(text, retry, legacy = false, allowFailure = false) {
  const receipt = evaluate(`new Promise(resolve => {
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
  let producer;
  try {
    producer = await guest(`/Users/admin/azad-insertion-probe${legacy ? '-before' : ''} ${retry ? '--allow-vm-brave-retry' : '--allow-vm-input'} ${quote(text)}`);
    producer.code = 0;
  } catch (error) {
    if (!allowFailure || !Number.isInteger(error.code)) throw error;
    producer = error;
  }
  const firstInput = await receipt;
  const trace = await evaluate(`({ value: document.querySelector('textarea[name=q],input[name=q]').value, events: window.azadEvents })`);
  return { producerExit: producer.code, producer: producer.stdout.trim(), handoff: producer.stderr.trim(), firstInput, ...trace };
}
function publish(mode, text, duringGap) {
  return new Promise((resolve, reject) => {
    const child = spawn('sshpass', [...ssh, `/Users/admin/clipboard-publication-gap ${mode} ${quote(text)}`], { env });
    let stdout = '', stderr = '', gapWork, failed;
    const deadline = setTimeout(() => { failed = new Error('Publication helper deadline; guest state is unknown.'); child.kill(); }, 15000);
    child.stdout.on('data', data => {
      stdout += data;
      if (!gapWork && stdout.includes('gap_open')) {
        gapWork = duringGap().then(() => {
          if (stdout.includes('write_ok')) throw new Error('Focus work did not finish within the observed publication gap.');
        }).catch(error => { failed = error; });
      }
    });
    child.stderr.on('data', data => { stderr += data; });
    child.on('error', error => { failed = error; });
    child.on('close', async code => {
      clearTimeout(deadline);
      await gapWork;
      if (failed) return reject(failed);
      if (code !== 0 || !gapWork) return reject(new Error(`Publication helper failed: ${code} ${stdout} ${stderr}`));
      resolve({ stdout: stdout.trim(), stderr: stderr.trim() });
    });
  });
}
try {
  if (!process.argv.includes('--ownership')) for (const mode of ['clear', 'declare']) {
    for (const restoreFocus of [false, true]) {
      const runId = randomUUID(), startedUtc = new Date().toISOString();
      const seed = 'AZAD_OLD_' + runId;
      const expected = 'AZAD_NEW_' + runId;
      await reset();
      const control = await paste(seed, false);
      if (control.value !== seed) throw new Error('The seed did not reach the actual Google field.');
      await reset();
      if (restoreFocus) await guest("open -a 'Jump Desktop'");
      const publication = await publish(mode, expected, async () => {
        if (restoreFocus) await focus();
      });
      const result = await paste(expected, true);
      const matches = result.value === expected;
      console.log(JSON.stringify({ runId, startedUtc, mode, restoreFocus, seedReceived: true,
        expected, publication, matches, ...result }));
      if (!matches) throw new Error('The Google field received missing or stale text.');
    }
  }
  if (process.argv.includes('--ownership')) {
    for (const legacy of [true, false]) {
      const runId = randomUUID(), startedUtc = new Date().toISOString();
      const expected = 'AZAD_NEW_' + runId, replacement = 'AZAD_OLD_' + runId;
      await reset();
      const control = await paste('AZAD_CONTROL_' + runId, false);
      if (control.value !== 'AZAD_CONTROL_' + runId) throw new Error('The positive control did not reach Google.');
      await reset();
      const interferer = spawn('sshpass', [...ssh,
        `/Users/admin/clipboard-interferer ${quote(expected)} ${quote(replacement)}`], { env });
      let stdout = '', stderr = '', failure;
      let armResolve;
      const armed = new Promise(resolve => { armResolve = resolve; });
      const done = new Promise(resolve => {
        const deadline = setTimeout(() => {
          failure = new Error('Interferer deadline; guest state is unknown.');
          armResolve(failure);
          interferer.kill();
        }, 15000);
        interferer.stdout.on('data', data => { stdout += data; if (stdout.includes('armed')) armResolve(null); });
        interferer.stderr.on('data', data => { stderr += data; });
        interferer.on('error', error => {
          clearTimeout(deadline);
          armResolve(error);
          resolve({ error });
        });
        interferer.on('close', code => {
          clearTimeout(deadline);
          const error = failure ?? (code !== 0 || !stdout.includes('replaced')
            ? new Error(`Interferer failed: ${code} ${stdout} ${stderr}`) : null);
          armResolve(error ?? new Error('The interferer ended before admission.'));
          resolve({ stdout: stdout.trim(), stderr: stderr.trim(), error });
        });
      });
      let result;
      try {
        const admissionError = await armed;
        if (admissionError) throw admissionError;
        result = await paste(expected, false, legacy, true);
      } catch (error) {
        await done;
        throw error;
      }
      const interference = await done;
      if (interference.error) throw interference.error;
      const producerResult = JSON.parse(result.producer).result;
      const observed = legacy
        ? producerResult === 'Pasted' && result.value === replacement
          && result.events.some(event => event.type === 'paste' && event.pasted === replacement)
        : producerResult === 'ClipboardWriteFailed' && result.producerExit !== 0
          && result.value === '' && result.events.length === 0;
      console.log(JSON.stringify({ runId, startedUtc, legacy, expected, replacement,
        positiveControlReceived: true, interference, expectedFaultOutcome: observed, ...result }));
      if (!observed) throw new Error('The ownership-race result does not match the admitted scenario.');
    }
  }
} finally {
  socket.close();
}
