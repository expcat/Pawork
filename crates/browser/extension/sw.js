// Pawork Browser Bridge service worker. Fixed action vocabulary only:
// tab lifecycle, navigation, bounded observation reads, selector click /
// type, and visible-tab screenshots. No arbitrary script evaluation, no
// page-to-extension bridge, no downloads or debugger attachment.
//
// NOTE: functions passed to chrome.scripting.executeScript are serialized
// in isolation, so readPage / clickElement / typeIntoElement must stay
// fully self-contained (no references to other top-level symbols).

const HOST_NAME = "dev.pawork.browser_bridge";
const EXTENSION_VERSION = "1.0.0";
let port = null;

function detectBrowser() {
  if (/Edg\//.test(navigator.userAgent)) {
    return "edge";
  }
  return "chrome";
}

async function isEnabled() {
  const stored = await chrome.storage.local.get("enabled");
  return stored.enabled === true;
}

function connect() {
  if (port) {
    return;
  }
  try {
    port = chrome.runtime.connectNative(HOST_NAME);
  } catch (error) {
    port = null;
    return;
  }
  port.onMessage.addListener(onHostMessage);
  port.onDisconnect.addListener(() => {
    port = null;
  });
  sendHello();
}

async function sendHello() {
  if (!port) {
    return;
  }
  try {
    port.postMessage({
      kind: "hello",
      browser: detectBrowser(),
      extension: EXTENSION_VERSION,
      enabled: await isEnabled(),
    });
  } catch (error) {
    port = null;
  }
}

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.alarms.create("pawork-keepalive", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);

chrome.storage.onChanged.addListener(async (changes, area) => {
  if (area !== "local" || !("enabled" in changes)) {
    return;
  }
  if (!port) {
    connect();
  } else {
    try {
      port.postMessage({ kind: "status", enabled: changes.enabled.newValue === true });
    } catch (error) {
      port = null;
    }
  }
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message && message.kind === "popup") {
    sendResponse({
      connected: !!port,
      browser: detectBrowser(),
    });
    return false;
  }
  if (message && message.kind === "popup_reconnect") {
    if (port) {
      try {
        port.disconnect();
      } catch (error) {
        /* already gone */
      }
      port = null;
    }
    connect();
    sendResponse({ connected: !!port });
    return false;
  }
});

function originOf(url) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return null;
    }
    const host = parsed.hostname.toLowerCase();
    const port = parsed.port;
    const defaultPort = parsed.protocol === "https:" ? "443" : "80";
    const shown = port && port !== defaultPort ? ":" + port : "";
    return parsed.protocol + "//" + host + shown;
  } catch (error) {
    return null;
  }
}

async function getTab(tabId) {
  try {
    return await chrome.tabs.get(tabId);
  } catch (error) {
    throw new Error("tab_gone: " + tabId);
  }
}

function checkOrigin(tab, expectOrigin) {
  const actual = originOf(tab.url);
  if (!actual || actual !== expectOrigin) {
    throw new Error("site_changed: " + (actual || "unknown"));
  }
}

// sol 审查⑥：归属按任务（Host 盖章的 run id）隔离，不是整浏览器
// 全局——任务 B 的 close 不能关任务 A 建的标签。
async function ownedSet(task) {
  const stored = await chrome.storage.session.get("owned_tasks");
  const map = stored.owned_tasks || {};
  return new Set(map[task] || []);
}

async function saveOwned(task, owned) {
  const stored = await chrome.storage.session.get("owned_tasks");
  const map = stored.owned_tasks || {};
  map[task] = [...owned];
  await chrome.storage.session.set({ owned_tasks: map });
}

function requireTask(params) {
  if (!params.task || typeof params.task !== "string") {
    throw new Error("missing task scope");
  }
  return params.task;
}

function waitForTab(tabId, timeoutMs) {
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const timer = setInterval(async () => {
      let tab;
      try {
        tab = await chrome.tabs.get(tabId);
      } catch (error) {
        clearInterval(timer);
        reject(new Error("tab_gone: " + tabId));
        return;
      }
      if (tab.status === "complete" || Date.now() - started > timeoutMs) {
        clearInterval(timer);
        resolve(tab);
      }
    }, 150);
  });
}

async function normalWindowId() {
  const windows = await chrome.windows.getAll();
  const normal = windows.filter((window) => window.type === "normal");
  if (normal.length === 0) {
    const created = await chrome.windows.create({ focused: false });
    return created.id;
  }
  const focused = normal.find((window) => window.focused);
  return (focused || normal[normal.length - 1]).id;
}

const handlers = {
  async tabs(params) {
    const task = requireTask(params);
    const owned = await ownedSet(task);
    const all = await chrome.tabs.query({});
    return {
      tabs: all.map((tab) => ({
        id: tab.id,
        url: tab.url || "",
        title: tab.title || "",
        active: tab.active,
        status: tab.status,
        owned: owned.has(tab.id),
      })),
    };
  },

  async tab_state(params) {
    const tab = await getTab(params.tab);
    return {
      id: tab.id,
      url: tab.url || "",
      title: tab.title || "",
      active: tab.active,
      status: tab.status,
    };
  },

  async new_tab(params) {
    const task = requireTask(params);
    const windowId = await normalWindowId();
    const created = await chrome.tabs.create({
      windowId,
      url: params.url || undefined,
      active: true,
    });
    const owned = await ownedSet(task);
    owned.add(created.id);
    await saveOwned(task, owned);
    let tab = created;
    if (params.url) {
      tab = await waitForTab(created.id, 12000);
    }
    return { tab: tab.id, url: tab.url || null, title: tab.title || "" };
  },

  async navigate(params) {
    const tab = await getTab(params.tab);
    if (params.require_owned) {
      const owned = await ownedSet(requireTask(params));
      if (!owned.has(params.tab)) {
        throw new Error("not agent-owned tab: attach it before navigating");
      }
    }
    if (params.bound_origin) {
      checkOrigin(tab, params.bound_origin);
    }
    await chrome.tabs.update(params.tab, { url: params.url });
    const settled = await waitForTab(params.tab, 12000);
    return {
      tab: settled.id,
      url: settled.url || "",
      title: settled.title || "",
      status: settled.status,
    };
  },

  async read(params) {
    const tab = await getTab(params.tab);
    checkOrigin(tab, params.expect_origin);
    const results = await chrome.scripting.executeScript({
      target: { tabId: params.tab },
      world: "ISOLATED",
      func: readPage,
      args: [params.expect_origin],
    });
    if (!results || !results[0] || results[0].result === undefined) {
      throw new Error("page read returned nothing");
    }
    if (results[0].result && results[0].result.site_changed !== undefined) {
      throw new Error("site_changed: " + results[0].result.site_changed);
    }
    return results[0].result;
  },

  async click(params) {
    const tab = await getTab(params.tab);
    checkOrigin(tab, params.expect_origin);
    const results = await chrome.scripting.executeScript({
      target: { tabId: params.tab },
      world: "ISOLATED",
      func: clickElement,
      args: [params.selector, params.expect_origin],
    });
    const outcome = results && results[0] && results[0].result;
    if (!outcome || outcome.error) {
      throw new Error((outcome && outcome.error) || "click returned nothing");
    }
    return outcome;
  },

  async type(params) {
    const tab = await getTab(params.tab);
    checkOrigin(tab, params.expect_origin);
    const results = await chrome.scripting.executeScript({
      target: { tabId: params.tab },
      world: "ISOLATED",
      func: typeIntoElement,
      args: [params.selector, params.text, params.expect_origin],
    });
    const outcome = results && results[0] && results[0].result;
    if (!outcome || outcome.error) {
      throw new Error((outcome && outcome.error) || "type returned nothing");
    }
    return outcome;
  },

  async screenshot(params) {
    // sol 复审③：监听必须先于首次异步状态读取安装——读到 A active 后、
    // windows.get 等待期间用户切到 B 的变化不能丢失。记录全部激活事件
    // （拿到目标 windowId 前无法过滤），目标窗口的任何激活变化——包括
    // 切回目标标签——都让本次捕获作废，宁可让调用方重试也不把 B 的
    // 像素标成 A。
    const activations = [];
    let navigated = false;
    const onActivated = (info) => {
      activations.push({ windowId: info.windowId, tabId: info.tabId });
    };
    const onUpdated = (tabId, change) => {
      if (tabId === params.tab && (change.url || change.pendingUrl)) {
        navigated = true;
      }
    };
    chrome.tabs.onActivated.addListener(onActivated);
    chrome.tabs.onUpdated.addListener(onUpdated);
    try {
      const tab = await getTab(params.tab);
      checkOrigin(tab, params.expect_origin);
      if (!tab.active) {
        throw new Error("tab is not the active tab of its window; make it visible first (no window focus change is allowed)");
      }
      const window = await chrome.windows.get(tab.windowId);
      if (window.state === "minimized") {
        throw new Error("browser window is minimized; screenshots need a visible window");
      }
      const startUrl = tab.url;
      const touched = () => activations.some((info) => info.windowId === tab.windowId);
      if (touched() || navigated) {
        throw new Error("screenshot target changed during capture; retry the action");
      }
      const dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, {
        format: "jpeg",
        quality: 55,
      });
      const after = await getTab(tab.id);
      if (touched() || navigated || !after.active || after.url !== startUrl) {
        throw new Error("screenshot target changed during capture; retry the action");
      }
      checkOrigin(after, params.expect_origin);
      const base64 = dataUrl.split(",", 2)[1] || "";
      return { jpeg_base64: base64, url: after.url, title: after.title };
    } finally {
      chrome.tabs.onActivated.removeListener(onActivated);
      chrome.tabs.onUpdated.removeListener(onUpdated);
    }
  },

  async close_tab(params) {
    const task = requireTask(params);
    const owned = await ownedSet(task);
    if (!owned.has(params.tab)) {
      throw new Error("not agent-owned: the agent may only close tabs this task created");
    }
    const tab = await getTab(params.tab);
    const url = tab.url;
    await chrome.tabs.remove(params.tab);
    owned.delete(params.tab);
    await saveOwned(task, owned);
    return { closed: params.tab, url };
  },

  async close_owned(params) {
    const task = requireTask(params);
    const owned = await ownedSet(task);
    const closed = [];
    for (const tabId of owned) {
      try {
        await chrome.tabs.remove(tabId);
        closed.push(tabId);
      } catch (error) {
        /* already gone */
      }
    }
    await saveOwned(task, new Set());
    return { closed };
  },
};

async function onHostMessage(message) {
  if (!message || message.kind !== "request") {
    return;
  }
  const { id, action, params } = message;
  try {
    // sol 审查⑨：桥侧串行锁后复查之外，扩展执行 handler 前再核对
    // 一次启用状态——等待期间用户关开关不得继续派发。
    if (!(await isEnabled())) {
      throw new Error("bridge is disabled; enable Pawork control in the extension toolbar popup");
    }
    const handler = handlers[action];
    if (!handler) {
      throw new Error("unsupported action: " + action);
    }
    const data = await handler(params || {});
    port.postMessage({ kind: "response", id, ok: true, data });
  } catch (error) {
    port.postMessage({
      kind: "response",
      id,
      ok: false,
      error: String((error && error.message) || error),
    });
  }
}

// ---- Fixed page scripts: each must be fully self-contained. ----

// sol 审查②：worker 侧检查 origin 与异步注入脚本之间存在窗口，跨源
// 导航会让脚本读到 / 操作未授权页面——每个页面函数访问 DOM 前先核
// 对 location.origin，不一致返回 site_changed 哨兵。
function readPage(expectOrigin) {
  if (location.origin !== expectOrigin) {
    return { site_changed: location.origin };
  }
  function selectorFor(el) {
    if (el.id) {
      return "#" + CSS.escape(el.id);
    }
    const name = el.getAttribute("name");
    if (name) {
      const attr = "[name=" + JSON.stringify(name) + "]";
      if (document.querySelectorAll(attr).length === 1) {
        return attr;
      }
    }
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && parts.length < 5) {
      let part = node.tagName.toLowerCase();
      if (node.id) {
        parts.unshift("#" + CSS.escape(node.id));
        break;
      }
      const parent = node.parentElement;
      if (parent) {
        const sameTag = Array.from(parent.children).filter((child) => child.tagName === node.tagName);
        if (sameTag.length > 1) {
          part += ":nth-of-type(" + (sameTag.indexOf(node) + 1) + ")";
        }
      }
      parts.unshift(part);
      if (!parent) {
        break;
      }
      node = parent;
    }
    return parts.join(" > ");
  }
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const text = (document.body && document.body.innerText) || "";
  const slice = (items, cap) => items.slice(0, cap);
  const links = slice(Array.from(document.querySelectorAll("a[href]")).filter(visible), 40).map((el) => ({
    selector: selectorFor(el),
    text: (el.innerText || "").slice(0, 120),
    href: el.href,
  }));
  const inputs = slice(
    Array.from(document.querySelectorAll("input, textarea, select")).filter(visible),
    40,
  ).map((el) => ({
    selector: selectorFor(el),
    kind: el.tagName.toLowerCase() + (el.type ? ":" + el.type : ""),
    name: el.name || "",
    placeholder: el.placeholder || "",
    value_preview: el.type === "password" ? "" : String(el.value || "").slice(0, 80),
  }));
  const buttons = slice(
    Array.from(document.querySelectorAll("button, [role=button], input[type=submit], input[type=button]")).filter(visible),
    40,
  ).map((el) => ({
    selector: selectorFor(el),
    text: (el.innerText || el.value || "").slice(0, 120),
  }));
  return {
    url: location.href,
    title: document.title.slice(0, 512),
    text: text.slice(0, 8192),
    truncated: text.length > 8192,
    links,
    inputs,
    buttons,
    viewport: { width: window.innerWidth, height: window.innerHeight },
  };
}

function clickElement(selector, expectOrigin) {
  if (location.origin !== expectOrigin) {
    return { error: "site_changed: " + location.origin };
  }
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return false;
    }
    const style = window.getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none";
  };
  try {
    const matches = Array.from(document.querySelectorAll(selector)).filter(visible);
    if (matches.length === 0) {
      return { error: "no visible element matches " + selector };
    }
    if (matches.length > 1) {
      return { error: "selector matches " + matches.length + " visible elements" };
    }
    const element = matches[0];
    if (element.tagName === "INPUT" && element.type === "file") {
      return { error: "file input clicks are not allowed" };
    }
    // sol 审查⑦：密码字段点击后的回执会带回 element.value 明文进入
    // 模型上下文与持久化事件（secret 红线）——fail-closed 直接拒绝。
    if (element.tagName === "INPUT" && element.type === "password") {
      return { error: "password input clicks are not allowed" };
    }
    element.scrollIntoView({ block: "center" });
    element.click();
    return {
      clicked: selector,
      tag: element.tagName,
      text: (element.innerText || element.value || "").slice(0, 120),
    };
  } catch (error) {
    return { error: String((error && error.message) || error) };
  }
}

function typeIntoElement(selector, text, expectOrigin) {
  if (location.origin !== expectOrigin) {
    return { error: "site_changed: " + location.origin };
  }
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return false;
    }
    const style = window.getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none";
  };
  try {
    const matches = Array.from(document.querySelectorAll(selector)).filter(visible);
    if (matches.length === 0) {
      return { error: "no visible element matches " + selector };
    }
    if (matches.length > 1) {
      return { error: "selector matches " + matches.length + " visible elements" };
    }
    const element = matches[0];
    const tag = element.tagName;
    if (tag !== "INPUT" && tag !== "TEXTAREA") {
      return { error: "selector targets a " + tag + "; only inputs and textareas can receive text" };
    }
    if (element.type === "password") {
      return { error: "password fields are not allowed" };
    }
    if (element.type === "file") {
      return { error: "file fields are not allowed" };
    }
    if (element.disabled || element.readOnly) {
      return { error: "element is disabled or read-only" };
    }
    element.scrollIntoView({ block: "center" });
    element.focus();
    const proto = tag === "TEXTAREA" ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
    setter.call(element, text);
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
    return { typed: selector, tag, value_length: text.length };
  } catch (error) {
    return { error: String((error && error.message) || error) };
  }
}

connect();
