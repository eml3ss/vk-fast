// VK Fast callback bridge — GPL-3.0-only.
// A one-time authorization response goes only to this fixed native URI.
const NATIVE_RETURN = "vkfast://oauth/callback";
const MAX_QUERY = 16_384;
const MAX_FIELD = 8_192;
const SECURITY_FIELDS = new Set(["code", "device_id", "state", "error", "error_description"]);
const FORBIDDEN_FIELDS = new Set([
  "accesstoken", "refreshtoken", "idtoken", "token", "authtoken",
  "bearertoken", "authorization", "authinfo", "silenttoken", "superapptoken",
  "password", "clientsecret", "codeverifier",
]);

function invalid() {
  return new Error("invalid_callback");
}
function forbidden(key) {
  return FORBIDDEN_FIELDS.has(key.toLowerCase().replaceAll(/[_-]/g, ""));
}
function safeField(value, limit = MAX_FIELD) {
  if (typeof value !== "string" || value.length === 0 || value.length > limit ||
      !/^[\x21-\x7e]+$/.test(value)) throw invalid();
  return value;
}

// JSON.parse discards duplicate keys. Preserve top-level names, including
// escaped spellings, after JSON.parse has already validated the full grammar.
function topLevelKeys(json) {
  const keys = [];
  let depth = 0;
  let expectingKey = false;
  for (let i = 0; i < json.length; i += 1) {
    const char = json[i];
    if (char === '"') {
      const start = i;
      for (i += 1; i < json.length; i += 1) {
        if (json[i] === "\\") i += 1;
        else if (json[i] === '"') break;
      }
      if (depth === 1 && expectingKey) {
        keys.push(JSON.parse(json.slice(start, i + 1)));
        expectingKey = false;
      }
    } else if (char === "{" || char === "[") {
      depth += 1;
      if (depth === 1) expectingKey = true;
    } else if (char === "}" || char === "]") {
      depth -= 1;
    } else if (char === "," && depth === 1) {
      expectingKey = true;
    }
  }
  return keys;
}
function rejectSecretFields(object) {
  const pending = [object];
  let visited = 0;
  while (pending.length) {
    const item = pending.pop();
    if (item === null || typeof item !== "object") continue;
    visited += 1;
    if (visited > 2_048) throw invalid();
    for (const [key, value] of Object.entries(item)) {
      if (forbidden(key)) throw invalid();
      if (value !== null && typeof value === "object") pending.push(value);
    }
  }
}
function responseFields(search) {
  if (typeof search !== "string" || !search.startsWith("?") ||
      search.length > MAX_QUERY || /%(?![0-9a-f]{2})/i.test(search)) throw invalid();
  const parameters = new URLSearchParams(search);
  const fields = new Map();
  let payload;
  for (const [key, value] of parameters) {
    if (forbidden(key)) throw invalid();
    if (key === "payload") {
      if (payload !== undefined) throw invalid();
      payload = value;
    } else if (SECURITY_FIELDS.has(key)) {
      if (fields.has(key)) throw invalid();
      fields.set(key, value);
    }
    // Provider metadata such as type=code_v2, lang and source is ignored.
    // No unknown field or supplied URL is copied into the native URI.
  }
  if (payload !== undefined) {
    if (fields.size !== 0 || payload.length > MAX_QUERY) throw invalid();
    let parsed;
    try { parsed = JSON.parse(payload); } catch { throw invalid(); }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) throw invalid();
    rejectSecretFields(parsed);
    for (const key of topLevelKeys(payload)) {
      if (SECURITY_FIELDS.has(key)) {
        if (fields.has(key)) throw invalid();
        fields.set(key, parsed[key]);
      }
    }
  }
  return fields;
}

/** Build the one fixed native return URI.
 * VK Fast checks expected state and PKCE; this page cannot authenticate them.
 */
export function buildNativeReturn(search, hash = "") {
  if (hash !== "") throw invalid();
  const fields = responseFields(search);
  const state = safeField(fields.get("state"));
  if (fields.has("error_description")) {
    const description = fields.get("error_description");
    if (typeof description !== "string" || description.length > 1_024) throw invalid();
  }
  const output = new URLSearchParams();
  if (fields.has("error")) {
    if (fields.has("code")) throw invalid();
    if (fields.has("device_id")) safeField(fields.get("device_id"));
    output.set("state", state);
    output.set("error", safeField(fields.get("error"), 128));
  } else {
    output.set("code", safeField(fields.get("code")));
    output.set("device_id", safeField(fields.get("device_id")));
    output.set("state", state);
  }
  const native = NATIVE_RETURN + "?" + output;
  if (native.length > MAX_QUERY) throw invalid();
  return native;
}

const TEXT = {
  ru: {
    heading: "Возвращаемся в VK Fast",
    message: "Разрешите браузеру открыть приложение VK Fast.",
    button: "Открыть VK Fast",
    status: "Если приложение не открылось, нажмите кнопку.",
    footnote: "Завершение входа проверит приложение VK Fast.",
    invalidHeading: "Не удалось передать ответ",
    invalidMessage: "Ссылка возврата неполная или недействительна. Начните вход заново в VK Fast.",
    invalidStatus: "Эту страницу можно закрыть.",
  },
  en: {
    heading: "Returning to VK Fast",
    message: "Allow your browser to open the VK Fast application.",
    button: "Open VK Fast",
    status: "If the application did not open, use the button.",
    footnote: "VK Fast will verify and complete the sign-in.",
    invalidHeading: "Could not return to the application",
    invalidMessage: "The return link is incomplete or invalid. Start sign-in again in VK Fast.",
    invalidStatus: "You can close this page.",
  },
};
const initialized = new WeakSet();

// Exported for lifecycle tests. No fetch, storage, cookie access or logging.
export function initializeBridge(win, doc, nav) {
  if (initialized.has(win)) return;
  initialized.add(win);
  const search = win.location.search;
  const hash = win.location.hash;
  let cleared = true;
  // Remove query and fragment before DOM work or external-protocol handoff.
  try { win.history.replaceState(null, "", win.location.pathname); }
  catch { cleared = false; }
  const language = (nav?.language || "en").toLowerCase().startsWith("ru") ? "ru" : "en";
  const text = TEXT[language];
  doc.documentElement.lang = language;
  const heading = doc.getElementById("heading");
  const message = doc.getElementById("message");
  const button = doc.getElementById("open-app");
  const status = doc.getElementById("status");
  const footnote = doc.getElementById("footnote");
  if (!heading || !message || !button || !status || !footnote) return;
  heading.textContent = text.heading;
  message.textContent = text.message;
  button.textContent = text.button;
  status.textContent = text.status;
  footnote.textContent = text.footnote;
  let native;
  try {
    if (!cleared || win.location.protocol !== "https:" || win.top !== win.self) throw invalid();
    native = buildNativeReturn(search, hash);
  } catch {
    heading.textContent = text.invalidHeading;
    message.textContent = text.invalidMessage;
    status.textContent = text.invalidStatus;
    button.disabled = true;
    button.hidden = true;
    return;
  }
  const openApplication = () => {
    try { win.location.assign(native); }
    catch {
      // An OS/browser restriction can prevent the automatic opening attempt.
      // Keep the manual button without printing the raw error or native URI.
      status.textContent = text.status;
    }
  };
  button.disabled = false;
  button.addEventListener("click", openApplication);
  openApplication(); // One automatic attempt; retries require a user click.
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  initializeBridge(window, document, navigator);
}
