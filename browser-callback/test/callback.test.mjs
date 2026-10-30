import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { buildNativeReturn, initializeBridge } from "../callback/callback.js";

const values = { code: "synthetic-once-code", device_id: "synthetic-device", state: "synthetic-state" };
const flat = "?" + new URLSearchParams(values);
const wrapped = (value) => "?" + new URLSearchParams({ payload: typeof value === "string" ? value : JSON.stringify(value) });

test("flat and JSON responses use the exact fixed native URI", () => {
  assert.equal(buildNativeReturn(flat), "vkfast://oauth/callback?code=synthetic-once-code&device_id=synthetic-device&state=synthetic-state");
  assert.equal(buildNativeReturn(wrapped(values)), buildNativeReturn(flat));
  assert.equal(buildNativeReturn(flat + "&type=code_v2&lang=en&source=web"), buildNativeReturn(flat));
  assert.equal(buildNativeReturn(flat + "&redirect_uri=https%3A%2F%2Fevil.example&next=https%3A%2F%2Fevil.example"), buildNativeReturn(flat));
  const uri = new URL(buildNativeReturn(flat));
  assert.equal(uri.protocol, "vkfast:");
  assert.equal(uri.hostname, "oauth");
  assert.equal(uri.pathname, "/callback");
});

test("opaque fields remain data after URL encoding", () => {
  const query = "?" + new URLSearchParams({ code: "code+with/=characters", device_id: "device=1", state: "state+slash/and%percent" });
  const uri = new URL(buildNativeReturn(query));
  assert.equal(uri.searchParams.get("code"), "code+with/=characters");
  assert.equal(uri.searchParams.get("device_id"), "device=1");
  assert.equal(uri.searchParams.get("state"), "state+slash/and%percent");
});

test("denial forwards state and error, never provider descriptions", () => {
  const query = "?" + new URLSearchParams({ state: values.state, error: "access_denied", error_description: "<script>private provider text</script>", type: "code_v2" });
  assert.equal(buildNativeReturn(query), "vkfast://oauth/callback?state=synthetic-state&error=access_denied");
  assert.equal(buildNativeReturn(wrapped({ state: values.state, error: "access_denied" })), buildNativeReturn(query));
});

test("missing, duplicated, mixed and malformed fields fail closed", () => {
  for (const query of [
    "", "code=x", "?state=x", "?code=x&state=x", "?code=x&device_id=x",
    flat + "&state=second", flat + "&code=second", flat + "&device_id=second",
    flat + "&error=access_denied", flat + "&payload=" + encodeURIComponent(JSON.stringify(values)),
    "?payload=%7Bbad-json", wrapped([]), wrapped(null), wrapped({ ...values, code: 123 }),
    "?code=%ZZ&device_id=x&state=x", "?code=x%0Aline&device_id=x&state=x",
    "?code=x&device_id=x&state=white+space", "?code=x&device_id=x&state=%C0%AF",
    "?state=x&error=access_denied&error_description=a&error_description=b",
    wrapped('{"code":"a","device_id":"b","state":"c","state":"d"}'),
    wrapped('{"code":"a","device_id":"b","state":"c","st\\u0061te":"d"}'),
  ]) assert.throws(() => buildNativeReturn(query), /invalid_callback/);
  assert.throws(() => buildNativeReturn(flat, "#access_token=never-forwarded"), /invalid_callback/);
});

test("access, refresh and related secrets are rejected in both formats", () => {
  for (const key of ["access_token", "refresh_token", "id_token", "token", "auth_token", "accessToken", "auth_info", "super_app_token", "client_secret", "code_verifier"]) {
    assert.throws(() => buildNativeReturn(flat + "&" + key + "=not-allowed"), /invalid_callback/);
    assert.throws(() => buildNativeReturn(wrapped({ ...values, [key]: "not-allowed" })), /invalid_callback/);
  }
  assert.throws(() => buildNativeReturn(wrapped({ ...values, metadata: { access_token: "not-allowed" } })), /invalid_callback/);
});

test("individual and overall size bounds apply before handoff", () => {
  assert.throws(() => buildNativeReturn("?" + new URLSearchParams({ ...values, code: "x".repeat(8193) })), /invalid_callback/);
  assert.throws(() => buildNativeReturn("?" + new URLSearchParams({ ...values, state: "x".repeat(8193) })), /invalid_callback/);
  assert.throws(() => buildNativeReturn(flat + "&ignored=" + "x".repeat(16384)), /invalid_callback/);
  assert.throws(() => buildNativeReturn(wrapped({ state: values.state, error: "x".repeat(129) })), /invalid_callback/);
});

function fixture({ search = flat, hash = "", protocol = "https:", framed = false, historyFails = false } = {}) {
  const calls = [];
  const elements = Object.fromEntries(["heading", "message", "open-app", "status", "footnote"].map((id) => [id, {
    textContent: "", disabled: true, hidden: false, listener: null,
    addEventListener(type, handler) { assert.equal(type, "click"); this.listener = handler; },
  }]));
  const doc = { documentElement: { lang: "" }, getElementById: (id) => elements[id] };
  const win = {
    location: {
      search, hash, protocol, pathname: "/vk-fast-auth/callback/",
      assign(uri) { calls.push(["assign", uri]); },
    },
    history: {
      replaceState(_state, _title, path) {
        calls.push(["clear", path]);
        if (historyFails) throw new Error("synthetic history failure");
        win.location.search = "";
        win.location.hash = "";
      },
    },
  };
  win.self = win;
  win.top = framed ? {} : win;
  return { calls, elements, doc, win };
}

test("address clearing precedes one automatic handoff, with manual retry", () => {
  const page = fixture();
  initializeBridge(page.win, page.doc, { language: "ru-RU" });
  assert.deepEqual(page.calls.map(([kind]) => kind), ["clear", "assign"]);
  assert.equal(page.calls[0][1], "/vk-fast-auth/callback/");
  assert.equal(page.win.location.search, "");
  assert.equal(page.win.location.hash, "");
  assert.equal(page.doc.documentElement.lang, "ru");
  assert.equal(page.elements["open-app"].disabled, false);
  for (const element of Object.values(page.elements)) {
    assert.ok(!element.textContent.includes(values.code));
    assert.ok(!element.textContent.includes(values.device_id));
    assert.ok(!element.textContent.includes(values.state));
  }
  initializeBridge(page.win, page.doc, { language: "ru-RU" });
  assert.equal(page.calls.length, 2);
  page.elements["open-app"].listener();
  assert.deepEqual(page.calls.map(([kind]) => kind), ["clear", "assign", "assign"]);
});

test("invalid, insecure and framed returns never hand off", () => {
  for (const options of [{ search: "?state=missing-code" }, { hash: "#refresh_token=not-allowed" }, { protocol: "http:" }, { framed: true }, { historyFails: true }]) {
    const page = fixture(options);
    initializeBridge(page.win, page.doc, { language: "en-US" });
    assert.deepEqual(page.calls.map(([kind]) => kind), ["clear"]);
    assert.equal(page.elements["open-app"].hidden, true);
    assert.equal(page.elements["open-app"].disabled, true);
    assert.equal(page.doc.documentElement.lang, "en");
  }
});

test("HTML has local assets, no-referrer and a restrictive CSP", async () => {
  const html = await readFile(new URL("../callback/index.html", import.meta.url), "utf8");
  assert.match(html, /name="referrer" content="no-referrer"/);
  assert.match(html, /connect-src 'none'/);
  assert.match(html, /default-src 'none'/);
  assert.match(html, /script-src 'self'/);
  assert.match(html, /style-src 'self'/);
  assert.doesNotMatch(html, /(?:src|href)="https?:/);
  assert.doesNotMatch(html, /onclick=/i);
});
