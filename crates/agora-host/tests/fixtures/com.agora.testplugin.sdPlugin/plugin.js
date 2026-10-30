// Minimal SDK plugin used by Agora's tests. Needs a Node with a global
// WebSocket (Node 22+).
const args = {};
for (let i = 2; i < process.argv.length; i += 2) args[process.argv[i].replace(/^-/, '')] = process.argv[i + 1];
const ws = new WebSocket('ws://127.0.0.1:' + args.port);
const send = (m) => ws.send(JSON.stringify(m));
ws.onopen = () => send({ event: args.registerEvent, uuid: args.pluginUUID });
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.event === 'willAppear') {
    send({ event: 'setTitle', context: m.context, payload: { title: 'hello', target: 0 } });
    send({ event: 'getGlobalSettings', context: args.pluginUUID });
  } else if (m.event === 'keyDown') {
    const count = ((m.payload.settings || {}).count || 0) + 1;
    send({ event: 'setSettings', context: m.context, payload: { count } });
    send({ event: 'setTitle', context: m.context, payload: { title: 'n=' + count } });
    send({ event: 'showOk', context: m.context });
    send({ event: 'setGlobalSettings', context: args.pluginUUID, payload: { pressed: true } });
  } else if (m.event === 'sendToPlugin') {
    send({ action: m.action, event: 'sendToPropertyInspector', context: m.context, payload: { echo: m.payload } });
  }
  send({ event: 'logMessage', payload: { message: 'got ' + m.event } });
};
