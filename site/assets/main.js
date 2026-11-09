// Helmp 落地页：语言切换与复制按钮。初始语言已由 <head> 里的内联脚本设好。
(function () {
  "use strict";

  var KEY = "helmp-lang";
  var TITLES = {
    en: "Helmp · macOS launcher with an agent switcher",
    zh: "Helmp · 会切换 Agent 会话的 macOS 启动器"
  };
  var root = document.documentElement;

  // localStorage 在隐私模式、禁用站点数据时可能抛错；失败就只在本页生效。
  function save(lang) {
    try {
      localStorage.setItem(KEY, lang);
    } catch (e) {}
  }

  function apply(lang) {
    root.setAttribute("data-lang", lang);
    root.lang = lang === "zh" ? "zh-CN" : "en";
    document.title = TITLES[lang];
    var buttons = document.querySelectorAll("[data-set-lang]");
    for (var i = 0; i < buttons.length; i++) {
      var on = buttons[i].getAttribute("data-set-lang") === lang;
      buttons[i].setAttribute("aria-pressed", on ? "true" : "false");
    }
  }

  apply(root.getAttribute("data-lang") === "zh" ? "zh" : "en");

  document.addEventListener("click", function (event) {
    var target = event.target;
    if (!(target instanceof Element)) return;

    var langButton = target.closest("[data-set-lang]");
    if (langButton) {
      var lang = langButton.getAttribute("data-set-lang") === "zh" ? "zh" : "en";
      apply(lang);
      save(lang);
      return;
    }

    var copyButton = target.closest("[data-copy-target]");
    if (copyButton) {
      var source = document.getElementById(copyButton.getAttribute("data-copy-target"));
      if (!source) return;
      copy(source.textContent || "", copyButton);
    }
  });

  function copy(text, button) {
    var done = function (ok) {
      button.classList.toggle("copied", ok);
      var label = ok ? { en: "Copied", zh: "已复制" } : { en: "Select and copy", zh: "请手动复制" };
      var en = button.querySelector('[lang="en"]');
      var zh = button.querySelector('[lang="zh-CN"]');
      if (en) en.textContent = label.en;
      if (zh) zh.textContent = label.zh;
      window.setTimeout(function () {
        if (en) en.textContent = "Copy";
        if (zh) zh.textContent = "复制";
        button.classList.remove("copied");
      }, 1600);
    };
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(
          function () { done(true); },
          function () { done(false); }
        );
        return;
      }
    } catch (e) {}
    done(false);
  }
})();
