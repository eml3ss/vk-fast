// Offline contracts derived from the supplied VK form attributes. No VK requests.
import assert from 'node:assert/strict';
const { installAuthBridge } = await import(new URL(`../src/tauri_login_bridge.mjs${new URL(import.meta.url).search}`, import.meta.url));
let documentNumber=0;
function fixture(host='vk.ru',path='/') {
  const values=new WeakMap(), timers=[];let nodes=[], clock=1000;
  class Element {
    constructor(tag,attrs={},label='') {
      this.tag=tag;this.attrs=attrs;this.textContent=label;this.isConnected=true;this.disabled=false;this.readOnly=false;
      this.type=attrs.type||'text';this.name=attrs.name||'';this.id=attrs.id||'';this.autocomplete=attrs.autocomplete||'';this.inputMode=attrs.inputmode||'';this.maxLength=attrs.maxlength||524288;this.clicks=0;
    }
    getAttribute(name){return this.attrs[name]??null;}
    getClientRects(){return this.hidden?[]:[this.getBoundingClientRect()];}
    getBoundingClientRect(){return this.rect||{width:320,height:364,x:0,y:0,left:0,top:0,right:320,bottom:364};}
    closest(selector){return selector==='form'?this.form:null;}
    querySelectorAll(selector){return select(selector,nodes.filter(n=>n.form===this));}
    click(){this.clicks++;this.onClick?.();}
    dispatchEvent(event){this.events??=[];this.events.push(event.type);return true;}
  }
  class Input extends Element {
    constructor(attrs){super('input',attrs);}
    get value(){throw Error('Bridge must never read input values');}
    set value(v){values.set(this,v);}
  }
  function matches(el,selector){
    const tag=selector.match(/^[a-z]+/i)?.[0];if(tag&&tag!==el.tag)return false;
    for(const m of selector.matchAll(/\[([^\]=*]+)(\*?=)?(?:"([^"]*)")?\]/g)){
      const v=el.attrs[m[1]]??(m[1] in el?el[m[1]]:undefined);
      if(!m[2] && v===undefined)return false;
      if(m[2]==='=' && String(v)!==m[3])return false;
      if(m[2]==='*=' && !String(v??'').includes(m[3]))return false;
    }
    return true;
  }
  function select(selector,candidates=nodes){return candidates.filter(el=>selector.split(',').some(part=>matches(el,part.trim())));}
  const document={querySelectorAll:selector=>select(selector)};
  const window={};window.top=window;window.HTMLInputElement=Input;
  for(const key of ['cookie','forms'])Object.defineProperty(document,key,{get(){throw Error('Sensitive DOM access');}});
  for(const key of ['localStorage','sessionStorage'])Object.defineProperty(window,key,{get(){throw Error('Browser storage access');}});
  const location={protocol:'https:',hostname:host,pathname:path,href:`https://${host}${path}?private=do-not-export`};
  installAuthBridge({window,document,location,getComputedStyle:el=>({display:'block',visibility:'visible',opacity:'1',...el.style}),HTMLInputElement:Input,Event:class{constructor(type){this.type=type}},setTimeout:(fn,delay=0)=>timers.push({fn,delay}),globalThis:{crypto:{randomUUID:()=>`fixture-${++documentNumber}`}},URL,now:()=>clock});
  const tick=()=>{const timer=timers.shift();if(timer){clock+=timer.delay;timer.fn();}};
  return {api:window.__vkFastAuth,Input,Element,values,set:list=>{nodes=list;},viewport:(width,height)=>{window.innerWidth=width;window.innerHeight=height;},advance:ms=>{clock+=ms;},tick,flush(){while(timers.length)tick();}};
}
function command(f,action,value=''){const s=f.api.snapshot();return {document:s.document,revision:s.revision,action,value};}
{
 const f=fixture(),entry=new f.Element('button',{'data-testid':'enter-another-way'},'Other sign-in options');f.set([entry]);
 assert.equal(f.api.snapshot().step,'entry');assert.equal(f.api.perform(command(f,'entry')),true);assert.equal(entry.clicks,1);
 const form=new f.Element('form',{name:'EnterPhoneOrMailForm'}),field=new f.Input({type:'tel',name:'login'}),submit=new f.Element('button',{'data-test-id':'submit_btn',type:'submit'},'Sign in');
 field.form=form;submit.form=form;f.set([form,field,submit]);
 const state=f.api.snapshot();assert.equal(state.step,'identifier');assert.ok(!JSON.stringify(state).includes('private'));
 const c=command(f,'submit','+1234567890');assert.equal(f.api.perform(c),true);f.flush();assert.equal(f.values.get(field),'+1234567890');assert.equal(submit.clicks,1);
 assert.equal(f.api.perform(c),false,'Pending or stale submissions must not duplicate');
}
{
 const f=fixture('id.vk.ru','/auth'),field=new f.Input({type:'password',autocomplete:'current-password'}),submit=new f.Element('button',{type:'submit'},'Continue');f.set([field,submit]);
 const state=f.api.snapshot();assert.equal(state.step,'password');assert.ok(!JSON.stringify(state).includes('value'));
 const old=command(f,'submit',`synthetic ' " password`);f.set([new f.Input({type:'password'}),submit]);assert.equal(f.api.perform(old),false);
 assert.equal(f.api.perform(command(f,'submit',`synthetic ' " password`)),true);f.flush();assert.equal(submit.clicks,1);
 const signup=new f.Input({type:'password',autocomplete:'new-password'});f.set([signup,submit]);assert.equal(f.api.snapshot().step,'manual');assert.equal(f.api.perform(command(f,'submit','no-registration')),false);
}
{
 const f=fixture('id.vk.ru','/auth'),field=new f.Input({type:'password',autocomplete:'current-password'}),submit=new f.Element('button',{type:'submit'},'Continue');
 submit.disabled=true;f.set([field,submit]);
 assert.equal(f.api.snapshot().step,'password');
 assert.equal(f.api.snapshot().busy,false,'An empty password disables Continue, not the native password field');
 assert.equal(f.api.perform(command(f,'submit','synthetic-password')),true);
 assert.equal(f.values.get(field),'synthetic-password');
 f.tick();f.tick();assert.equal(submit.clicks,0,'Wait for website validation without bypassing disabled');
 submit.disabled=false;f.flush();assert.equal(submit.clicks,1);
}
{
 for(const change of ['replacement','captcha','locked','validation-failed']) {
  const f=fixture('id.vk.ru','/auth'),field=new f.Input({type:'password'}),submit=new f.Element('button',{type:'submit'},'Continue');
  submit.disabled=true;f.set([field,submit]);
  assert.equal(f.api.perform(command(f,'submit','synthetic-password')),true);f.tick();
  if(change==='replacement')f.set([new f.Input({type:'password'}),submit]);
  else if(change==='captcha')f.set([field,submit,new f.Element('iframe',{src:'https://id.vk.ru/not_robot_captcha'})]);
  else if(change==='locked')field.readOnly=true;
  else {f.flush();assert.equal(submit.clicks,0);}
  submit.disabled=false;f.flush();assert.equal(submit.clicks,0,'No late submission after '+change);
 }
}
{
 for(const lock of ['disabled','readonly','request']) {
  const f=fixture('id.vk.ru','/auth'),form=new f.Element('form'),field=new f.Input({type:'password'}),submit=new f.Element('button',{type:'submit'},'Continue');
  field.form=submit.form=form;f.set([form,field,submit]);
  if(lock==='disabled')field.disabled=true;
  if(lock==='readonly')field.readOnly=true;
  if(lock==='request')form.attrs['aria-busy']='true';
  assert.equal(f.api.snapshot().busy,true);assert.equal(f.api.perform(command(f,'submit','synthetic-password')),false);assert.equal(f.values.get(field),undefined);
 }
}
{
 for(const retry of [false,true]) {
  const f=fixture('id.vk.ru','/auth'),field=new f.Input({type:'password'}),submit=new f.Element('button',{type:'submit'},'Continue');
  submit.disabled=true;f.set([field,submit]);
  assert.equal(f.api.perform(command(f,'submit','old-synthetic-password')),true);f.tick();f.advance(8100);
  if(retry)assert.equal(f.api.perform(command(f,'submit','new-synthetic-password')),true);
  submit.disabled=false;f.flush();
  assert.equal(submit.clicks,retry?1:0,'Throttled timers cannot submit an expired or superseded attempt');
 }
}
{
 const f=fixture('id.vk.ru','/auth'),fields=Array.from({length:6},()=>new f.Input({type:'tel',maxlength:1}));f.set(fields);
 assert.equal(f.api.snapshot().step,'code');assert.equal(f.api.snapshot().code_length,6);
 assert.equal(f.api.perform(command(f,'submit','012345')),true);f.flush();assert.deepEqual(fields.map(el=>f.values.get(el)),['0','1','2','3','4','5']);
}
{
 const f=fixture('id.vk.ru','/auth'),frame=new f.Element('iframe',{src:'https://id.vk.ru/not_robot_captcha?private=do-not-export'}),field=new f.Input({type:'password'}),submit=new f.Element('button',{type:'submit'},'Continue');f.set([frame,field,submit]);
 const state=f.api.snapshot();assert.equal(state.step,'captcha');assert.equal(state.width,368);assert.equal(state.height,462);
 assert.equal(f.api.perform(command(f,'submit','not-submitted')),false);assert.equal(f.api.perform(command(f,'entry')),false);assert.equal(submit.clicks,0);assert.equal(frame.clicks,0);assert.ok(!JSON.stringify(state).includes('private'));
}
{
 const f=fixture(),search=new f.Input({type:'search',id:'search_top_input'});f.set([search]);assert.equal(f.api.snapshot().step,'loading');
 assert.equal(f.api.perform(command(f,'submit','must-not-search')),false);
 assert.equal(fixture('vk.ru.example.test').api,undefined);
}
{
 const f=fixture(), entry=new f.Element('button',{'data-testid':'enter-another-way'},'Other sign-in options');entry.disabled=true;f.set([entry]);
 assert.equal(f.api.snapshot().busy,true);assert.equal(f.api.perform(command(f,'entry')),false);assert.equal(entry.clicks,0);
 entry.disabled=false;assert.equal(f.api.perform(command(f,'entry')),true);assert.equal(entry.clicks,1);
}
{
 const f=fixture('id.vk.ru','/auth'),field=new f.Input({type:'password'}),submit=new f.Element('button',{type:'submit'},'Continue'),other=new f.Element('button',{},'Other options');f.set([field,submit,other]);
 const revision=f.api.snapshot().revision;f.set([field,submit,new f.Element('button',{},'Other options')]);
 assert.equal(f.api.snapshot().revision,revision,'Method button rerenders must not erase a native draft');
 const consent=new f.Input({type:'checkbox',name:'terms_agreement'});consent.required=true;f.set([field,submit,consent]);
 assert.equal(f.api.snapshot().step,'manual');assert.equal(f.api.perform(command(f,'submit','no-consent-click')),false);assert.equal(consent.clicks,0);
}
{
 const f=fixture(),form=new f.Element('form',{name:'EnterPhoneOrMailForm'}),phone=new f.Input({type:'tel',name:'login'}),email=new f.Input({type:'text',autocomplete:'username'}),tab=new f.Element('button',{'data-test-id':'email-id'},'Email'),submit=new f.Element('button',{'data-test-id':'submit_btn',type:'submit'},'Sign in');
 phone.form=email.form=submit.form=form;email.hidden=true;tab.onClick=()=>{phone.hidden=true;email.hidden=false;};f.set([form,phone,email,tab,submit]);
 assert.equal(f.api.perform(command(f,'submit','user@example.test')),true);f.flush();assert.equal(f.values.get(email),'user@example.test');assert.equal(f.values.get(phone),undefined);assert.equal(submit.clicks,1);
}
{
 for(const concealment of ['hidden-wrapper','aria-hidden','inert','transparent','offscreen']) {
  const f=fixture('id.vk.ru','/auth');
  const form=new f.Element('form'),wrapper=new f.Element('div');
  const autofill=new f.Input({name:'username',autocomplete:'username'});
  const field=new f.Input({type:'password',name:'password',autocomplete:'current-password'});
  const submit=new f.Element('button',{type:'submit'},'Continue');
  autofill.parentElement=wrapper;
  autofill.form=field.form=submit.form=form;
  if(concealment==='hidden-wrapper')wrapper.attrs.hidden='';
  if(concealment==='aria-hidden')wrapper.attrs['aria-hidden']='true';
  if(concealment==='inert')wrapper.inert=true;
  if(concealment==='transparent')wrapper.style={opacity:'0'};
  if(concealment==='offscreen')autofill.rect={width:320,height:30,left:-10000,top:0,right:-9680,bottom:30};
  f.set([form,wrapper,autofill,field,submit]);
  assert.equal(f.api.snapshot().step,'password',concealment+' must not turn the password screen into a mixed form');
  assert.equal(f.api.perform(command(f,'submit','synthetic-password')),true);
  f.flush();
  assert.equal(f.values.get(autofill),undefined);
  assert.equal(f.values.get(field),'synthetic-password');
  assert.equal(submit.clicks,1);
 }
}
{
 const f=fixture('id.vk.ru','/auth'),field=new f.Input({type:'text',autocomplete:'current-password'}),submit=new f.Element('button',{type:'submit'},'Continue');
 f.set([field,submit]);
 assert.equal(f.api.snapshot().step,'password','The website eye toggle must retain password semantics');
 assert.equal(f.api.perform(command(f,'submit','synthetic-password')),true);f.flush();
 assert.equal(submit.clicks,1);
 const username=new f.Input({autocomplete:'username'});f.set([username,field,submit]);
 assert.equal(f.api.snapshot().step,'manual','A genuinely visible mixed form remains unsupported');
}
{
 const f=fixture('id.vk.ru','/auth'),widget=new f.Element('div',{'data-test-id':'captcha-widget'});
 f.viewport(0,0);widget.rect={width:0,height:176,left:8,top:8,right:8,bottom:184};f.set([widget]);
 assert.equal(f.api.snapshot().step,'captcha','Hidden WebKit has no viewport yet, but its CAPTCHA must still be shown');
 assert.equal(f.api.perform(command(f,'submit','not-a-code')),false);
 const hidden=new f.Element('div',{hidden:''});widget.parentElement=hidden;
 assert.notEqual(f.api.snapshot().step,'captcha','An explicitly hidden widget remains hidden even before first show');
}
{
  for (const type of ['tel', 'password']) {
    const f = fixture(type === 'password' ? 'id.vk.ru' : 'vk.ru', '/');
    f.viewport(340, 400);
    const entry = new f.Element('button', {'data-testid': 'enter-another-way'}, 'Other sign-in options');
    entry.rect = {width: 270, height: 40, left: 440, top: 460, right: 710, bottom: 500};
    f.set([entry]);
    assert.equal(f.api.snapshot().step, 'entry', 'A hidden desktop layout can place VK controls outside the small webview');
    assert.equal(f.api.perform(command(f, 'entry')), true);
    const form = new f.Element('form');
    const field = new f.Input({type, name: type === 'password' ? 'password' : 'login'});
    const submit = new f.Element('button', {type: 'submit'}, 'Continue');
    field.form = form;
    submit.form = form;
    field.rect = {width: 270, height: 40, left: 440, top: 520, right: 710, bottom: 560};
    submit.rect = {width: 270, height: 40, left: 440, top: 580, right: 710, bottom: 620};
    f.set([form, field, submit]);
    assert.equal(f.api.snapshot().step, type === 'password' ? 'password' : 'identifier');
    const value = type === 'password' ? 'synthetic-offscreen-password' : '+1234567890';
    assert.equal(f.api.perform(command(f, 'submit', value)), true);
    f.flush();
    assert.equal(f.values.get(field), value);
    assert.equal(submit.clicks, 1);
    field.style = {display: 'none'};
    assert.equal(f.api.snapshot().step, 'loading', 'CSS-hidden controls must stay excluded');
  }
}
{
  const f = fixture('id.vk.ru', '/auth');
  const widget = new f.Element('div', {'data-test-id': 'captcha-widget'});
  widget.getClientRects = () => [];
  f.set([widget]);
  f.viewport(0, 0);
  assert.equal(f.api.snapshot().step, 'captcha', 'A hidden webview may not have any layout rectangles yet');
  f.viewport(340, 400);
  assert.equal(f.api.snapshot().step, 'loading', 'A real non-rendered element remains excluded');
  f.viewport(0, 0);
  widget.parentElement = new f.Element('div', {'aria-hidden': 'true'});
  assert.equal(f.api.snapshot().step, 'loading');
}
console.log('Offline login bridge contracts passed.');
