// Run with: node mobile/scripts/test-feishu-callback.cjs
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const html = fs.readFileSync(path.join(__dirname, '../../crates/vpn-server/src/handlers/feishu_callback.html'), 'utf8');
const script = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map(m => m[1]).join('\n');
function page(ok, android, ua = 'Android') {
  const timers = [], listeners = {}, nodes = {};
  for (const id of ['return', 'close', 'hint']) nodes[id] = {href:'yilian://auth/feishu-return', addEventListener(type, fn) {this[type] = fn;}};
  let closed = 0, stripped = 0;
  const document = {body:{dataset:{result:ok ? 'success':'failure', android:String(android)}}, hidden:false,
    getElementById:id => nodes[id], addEventListener:(name, fn)=> listeners[name]=fn,
    createElement:()=> ({}), head:{appendChild(){}}};
  const location = {pathname:'/api/v1/auth/feishu/callback', href:'https://vpn.example/callback?code=secret'};
  const window = {close:()=> closed++, setTimeout:fn=>timers.push(fn)};
  vm.runInNewContext(script,{window,document,location,navigator:{userAgent:ua},history:{replaceState:(_,__,url)=>{assert.equal(url, location.pathname);stripped++;}}});
  return {nodes,document,window,location,timers,listeners,closed:()=>closed,stripped:()=>stripped};
}
const mobile = page(true,true);
assert.equal(mobile.stripped(),1);
assert.equal(mobile.closed(),0);
mobile.timers.forEach(fn=>fn());
assert.equal(mobile.location.href,'yilian://auth/feishu-return');
assert.equal(mobile.closed(),0); // Don't strand the user by closing before app launch.
mobile.document.hidden=true;
mobile.listeners.visibilitychange();
assert.equal(mobile.closed(),1);
const failed=page(false,false);
assert.equal(failed.timers.length,0);
failed.nodes.return.click();
assert.equal(failed.closed(),0);
failed.nodes.close.click();
assert.equal(failed.closed(),1);
const desktop=page(true,false,'Desktop');
assert.equal(desktop.nodes.return.hidden,true);
desktop.timers.forEach(fn=>fn());
assert.equal(desktop.closed(),1);
assert.equal(desktop.location.href,'https://vpn.example/callback?code=secret');
const feishu=page(true,true,'Android Lark');
let bridgeClosed=0;
feishu.window.tt={closeWindow:()=>bridgeClosed++};
feishu.nodes.return.click();feishu.document.hidden=true;feishu.listeners.visibilitychange();
assert.equal(bridgeClosed,1);
assert.equal(feishu.closed(),0);
console.log('Callback behavior passed: Android return, blocked-launch fallback, failure, desktop close, Feishu close, URL cleanup.');
