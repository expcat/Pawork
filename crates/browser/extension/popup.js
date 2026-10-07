const status = document.getElementById("status");
const enable = document.getElementById("enable");
const reconnect = document.getElementById("reconnect");

async function refresh() {
  chrome.runtime.sendMessage({ kind: "popup" }, (reply) => {
    if (chrome.runtime.lastError || !reply) {
      status.textContent = "未连接 Pawork / Not connected to Pawork";
      return;
    }
    const browser = reply.browser === "edge" ? "Edge" : "Chrome";
    status.textContent = reply.connected
      ? browser + " 桥已连接，等待 Desktop 动作 / bridge connected, waiting for Desktop actions"
      : "Pawork 桥未连接（点重新连接）/ Pawork bridge not connected (press Reconnect)";
  });
  const stored = await chrome.storage.local.get("enabled");
  enable.checked = stored.enabled === true;
}

enable.addEventListener("change", async () => {
  await chrome.storage.local.set({ enabled: enable.checked });
});

reconnect.addEventListener("click", () => {
  chrome.runtime.sendMessage({ kind: "popup_reconnect" }, () => {
    void chrome.runtime.lastError;
    refresh();
  });
});

refresh();
