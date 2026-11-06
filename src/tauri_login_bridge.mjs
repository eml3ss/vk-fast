// Native-driven presentation of VK's own forms. Never reads input values or
// auth objects, and never clicks a CAPTCHA, agreement, or registration control.
export function installAuthBridge({window, document, location, getComputedStyle, HTMLInputElement, Event, setTimeout, globalThis, URL, now=()=>Date.now()}) {
  if (window.top !== window) return;
  const hosts = ['vk.ru','vk.com','id.vk.ru','id.vk.com','login.vk.ru','login.vk.com'];
  if (location.protocol !== 'https:' || !hosts.includes(location.hostname)) return;
  if (window.__vkFastAuth) return;
  const documentId = globalThis.crypto?.randomUUID?.() || `doc-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const ids = new WeakMap(); let nextId=0, revision=0, signature='', formSignature='', pending=0, pendingSignature='', attempt=0;
  const id = node => { if (!node) return 0; if (!ids.has(node)) ids.set(node,++nextId); return ids.get(node); };
  const visible = el => {
    if(!el || !el.isConnected)return false;
    // VK keeps autofill username inputs inside a hidden wrapper on password
    // screens. CSS can give those inputs a box; they are not another login step.
    for(let node=el;node;node=node.parentElement) {
      const style=getComputedStyle(node);
      if(node.hidden || node.inert || node.getAttribute('hidden')!==null || node.getAttribute('aria-hidden')==='true'
        || style.display==='none' || style.visibility==='hidden' || style.visibility==='collapse' || style.opacity==='0')return false;
    }
    // A hidden native webview can have no viewport or layout boxes yet.
    if(window.innerWidth===0 || window.innerHeight===0)return true;
    if(!el.getClientRects().length)return false;
    const rect=el.getBoundingClientRect();
    // Native fields drive the real desktop form without pointer hit-testing.
    // Its rendered controls can sit outside the small hidden browser viewport.
    return rect.width>0 && rect.height>0 && rect.right>0 && rect.bottom>0;
  };
  const all = (selector,scope=document) => Array.from(scope.querySelectorAll(selector)).filter(visible);
  const text = el => (el?.getAttribute('aria-label') || el?.innerText || el?.textContent || '').replace(/\s+/g,' ').trim();
  const byText = (re,scope=document,enabledOnly=false) => all('button,a,[role="button"]',scope).find(el=>re.test(text(el)) && (!enabledOnly || (!el.disabled && el.getAttribute('aria-disabled')!=='true')));
  const bySelector = selector => all(selector)[0];
  const disabled = el => !!el?.disabled || el?.getAttribute('aria-disabled')==='true';
  const locked = info => info.scope?.getAttribute?.('aria-busy')==='true' || info.submit?.getAttribute('aria-busy')==='true' || info.fields.some(el=>disabled(el)||el.readOnly);
  const authRoute = () => location.hostname.startsWith('id.') || location.hostname.startsWith('login.') || location.pathname==='/' || /^\/(?:login|auth|authcheck|validate)(?:\.php|\/.*)?$/.test(location.pathname);
  function captcha() {
    const widget=bySelector('[data-test-id="captcha-widget"]');
    const frame=all('iframe').find(el=>{
      try {return /\/(?:not_robot_captcha|captcha)(?:\.|\/|$)/i.test(new URL(el.getAttribute('src')||'',location.href).pathname);} catch {return false;}
    });
    const checkbox=bySelector('[data-test-id="not-robot-captcha-checkbox-input"]');
    if (!(widget||frame||checkbox)) return null;
    let card=checkbox?.closest('[role="dialog"]');
    try { card ||= frame?.contentDocument?.querySelector('[role="dialog"]'); } catch {}
    const box=card?.getBoundingClientRect();
    return {node:widget||frame||checkbox,width:Math.round(Math.max(340,Math.min(620,(box?.width||336)+32))),height:Math.round(Math.max(400,Math.min(780,(box?.height||430)+32)))};
  }
  function inspect() {
    const cap=captcha(); if(cap)return {step:'captcha',fields:[],cap};
    if(!authRoute())return {step:'loading',fields:[]};
    const entry=bySelector('[data-testid="enter-another-way"], [data-testid="add-another-account"]');
    const fields=all('input').filter(el=>!['hidden','search','checkbox','radio','button','submit'].includes(el.type));
    if(fields.some(el=>el.autocomplete==='new-password'))return {step:'manual',fields:[]};
    const passwords=fields.filter(el=>el.type==='password' || el.autocomplete==='current-password');
    const code=fields.filter(el=>{
      const name=[el.name,el.id,el.autocomplete,el.getAttribute('aria-label'),el.getAttribute('placeholder')].join(' ');
      return !/country|dial.?code|prefix/i.test(name) && (/one-time-code|(?:^|[_ -])(?:otp|code|pin)(?:[_ -]|$)|код/i.test(name) || (el.inputMode==='numeric' && el.maxLength>=4 && el.maxLength<=8));
    });
    const slots=fields.filter(el=>el.maxLength===1 && ['tel','text','number'].includes(el.type));
    const identifier=fields.find(el=>el.autocomplete==='username' || el.autocomplete==='tel' || el.type==='email' || el.type==='tel' || /^(login|email|phone|username)$/.test(el.name||''));
    let step, active=[];
    if(passwords.some(el=>!code.includes(el)) && (code.length || (identifier && !identifier.readOnly)))return {step:'manual',fields:[]};
    if(code.length) {step='code';active=code;}
    else if(passwords.length===1) {step='password';active=passwords;}
    else if(slots.length>=4 && slots.length<=8) {step='code';active=slots;}
    else if(identifier) {step='identifier';active=[identifier];}
    else if(entry) {step='entry';}
    else {
      const heading=location.hostname.startsWith('id.') || location.hostname.startsWith('login.') ? all('h1,h2,[role="heading"]').map(text).join(' ') : '';
      step=/подтверд|check your|confirm|approve|notification|уведомлен/i.test(heading)?'approval':(fields.length?'manual':'loading');
    }
    const scope=active[0]?.closest('form') || document;
    const submit=all('[data-test-id="submit_btn"],button[type="submit"],input[type="submit"]',scope)[0]
      || byText(/^(?:Продолжить|Войти|Далее|Подтвердить|Continue|Sign in|Log in|Next|Confirm)$/i,scope);
    if(step==='code' && active.length===1 && !submit && active[0].autocomplete!=='one-time-code')return {step:'manual',fields:[]};
    const password=step==='password'?null:byText(/^(?:Пароль|По паролю|Войти с паролем|Войти по паролю|Войти с помощью пароля|Использовать пароль|Ввести пароль|Вход по паролю|Password|Enter password|Use password|Sign in with password|Log in with password)$/i,document,true);
    const other=byText(/^(?:Другой способ|Другие способы|Другие способы входа|Войти другим способом|Попробовать другой способ|Выбрать другой способ|Other sign-in options|Try another way|Try another method|Other options|Other ways|Use another method)$/i,document,true);
    const back=byText(/^(?:Назад|Вернуться|Back|Go back)$/i,document,true);
    const consent=all('input[type="checkbox"]',scope).some(el=>el.getAttribute('data-test-id')!=='save_user_chk' && (el.required || /agree|consent|terms|policy|offer/i.test([el.name,el.id].join(' '))));
    const registration=/зарегистр|создать (?:новый )?аккаунт|регистрация|sign up|create (?:an? |your |new )?account/i;
    if(consent || (submit && registration.test(text(submit))))return {step:'manual',fields:[]};
    const error=all('[aria-invalid="true"],[role="alert"],[data-test-id*="error"],[data-testid*="error"],[class*="FormItem__statusError"],[class*="TextField__error"]',scope).length>0;
    if(step==='loading' && (password||other))step='methods';
    return {step,fields:active,entry,submit,password,other,back,error,scope};
  }
  function snapshot() {
    const info=inspect();
    formSignature=[info.step,...info.fields.map(el=>`${id(el)}-${el.maxLength}`),id(info.entry),id(info.cap?.node)].join(':');
    const next=`${formSignature}:${!!info.error}`;
    if(next!==signature){signature=next;revision++;}
    const elapsed=now()-pending;
    // Continue is also disabled for an empty/invalid field. It must not prevent
    // filling that field; only an explicit request or locked form is busy.
    const busy=(pending && (elapsed<250 || (formSignature===pendingSignature && !info.error && elapsed<8000))) || disabled(info.entry) || locked(info);
    let length=info.fields.length>1?info.fields.length:info.fields[0]?.maxLength;
    if(!(length>=4&&length<=16))length=6;
    return {document:documentId,revision,step:info.step,busy:Boolean(busy),error:!!info.error,code_length:info.step==='code'?length:0,password_option:!!info.password,other_option:!!info.other,back_option:!!info.back,width:info.cap?.width||0,height:info.cap?.height||0};
  }
  const setValue = (el,value) => {
    const descriptor=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value');
    descriptor.set.call(el,value);
    el.dispatchEvent(new Event('input',{bubbles:true}));
    el.dispatchEvent(new Event('change',{bubbles:true}));
  };
  function perform(command) {
    const current=snapshot();
    if(current.document!==command.document || current.revision!==command.revision || current.busy || current.step==='captcha')return false;
    const info=inspect();
    if(command.action==='entry' && info.entry){attempt++;info.entry.click();return true;}
    const alternative={password:info.password,other:info.other,back:info.back}[command.action];
    if(alternative){attempt++;pending=now();pendingSignature=formSignature;alternative.click();return true;}
    if(command.action!=='submit' || !['identifier','password','code'].includes(info.step) || typeof command.value!=='string' || command.value.length>512 || !command.value)return false;
    if(info.step==='code' && (!/^\d+$/.test(command.value) || command.value.length!==current.code_length))return false;
    let value=command.value;command.value='';
    pending=now();pendingSignature=formSignature;
    const activeAttempt=++attempt, submitBy=now()+5000;
    const fill=()=>{
      if(activeAttempt!==attempt || now()>submitBy){value='';return;}
      const currentForm=inspect();
      if(currentForm.step!==info.step || captcha() || !authRoute()){pending=0;value='';return;}
      if(currentForm.fields.length>1 && currentForm.step==='code')currentForm.fields.forEach((el,index)=>setValue(el,value[index]||''));
      else if(currentForm.fields[0])setValue(currentForm.fields[0],value);
      value='';
      const filled=currentForm.fields.slice();
      const submitWhenReady=remaining=>{
        if(activeAttempt!==attempt || now()>submitBy)return;
        const next=inspect();
        if(next.step!==info.step || captcha() || !authRoute() || locked(next) || next.fields.length!==filled.length || next.fields.some((el,index)=>el!==filled[index]))return;
        // OTP widgets can submit themselves after the last digit.
        if(next.submit) {
          if(!disabled(next.submit))next.submit.click();
          // React validation may enable Continue after the input event's tick.
          // Retry only the button, never the credentials or a replacement form.
          else if(remaining>0 && !next.error)setTimeout(()=>submitWhenReady(remaining-1),100);
        }
        else if(next.step!=='code' && next.fields[0]?.form?.requestSubmit)next.fields[0].form.requestSubmit();
      };
      setTimeout(()=>submitWhenReady(50),0);
    };
    if(info.step==='identifier'){
      const tab=bySelector(value.includes('@')?'[data-test-id="email-id"]':'[data-test-id="phone-id"]');
      if(tab)tab.click();
      setTimeout(fill,0);
    } else fill();
    return true;
  }
  window.__vkFastAuth={snapshot,perform};
}
