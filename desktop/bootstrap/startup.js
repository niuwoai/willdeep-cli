const translations = {
  zh: ["正在连接本机 Runtime…", "无法连接本机 Runtime。请退出后重新打开 WillDeep Desktop。"],
  ja: ["ローカルランタイムに接続中…", "ローカルランタイムに接続できません。WillDeep Desktop を再起動してください。"],
  en: ["Connecting to the local Runtime…", "Could not connect to the local Runtime. Quit and open WillDeep Desktop again."],
};
const [waiting, failure] = translations[navigator.language.split("-")[0]] ?? translations.en;
const status = document.getElementById("status");
status.textContent = waiting;
window.willdeepStartupFailed = () => { status.textContent = failure; };
