// Helmp 落地页：语言切换、复制按钮、首屏截图切换。初始语言已由 <head> 里的内联脚本设好。
(function () {
  "use strict";

  var KEY = "helmp-lang";
  // 子页面在 <body data-title-en="…" data-title-zh="…"> 上写自己的标题；没写就用首页标题。
  var body = document.body;
  var TITLES = {
    en: (body && body.getAttribute("data-title-en")) || "Helmp · macOS launcher with an agent switcher",
    zh: (body && body.getAttribute("data-title-zh")) || "Helmp · 会切换 Agent 会话的 macOS 启动器"
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

  // 首屏截图切换：WAI-ARIA tabs（左右方向键、Home / End 切换，自动激活）。不会自己轮播。
  var stage = document.querySelector("[data-stage]");
  if (stage) {
    var tabs = Array.prototype.slice.call(stage.querySelectorAll('[role="tab"]'));

    var select = function (tab, focus) {
      for (var i = 0; i < tabs.length; i++) {
        var t = tabs[i];
        var on = t === tab;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
        var panel = document.getElementById(t.getAttribute("aria-controls"));
        if (panel) {
          if (on && panel.hidden) {
            panel.hidden = false;
            panel.classList.remove("is-entering");
            void panel.offsetWidth; // 重新触发淡入
            panel.classList.add("is-entering");
          } else if (!on) {
            panel.hidden = true;
          }
          var caption = document.getElementById(panel.getAttribute("aria-describedby"));
          if (caption) caption.hidden = !on;
        }
      }
      if (focus) tab.focus();
      // 窄屏时标签条可以横向滚动：把选中的那个滚进可视范围（不动页面本身的滚动）
      var bar = tab.parentElement;
      if (bar && bar.scrollWidth > bar.clientWidth) {
        var left = tab.offsetLeft - (bar.clientWidth - tab.offsetWidth) / 2;
        bar.scrollLeft = Math.max(0, left);
      }
    };

    stage.addEventListener("click", function (event) {
      var target = event.target;
      if (!(target instanceof Element)) return;
      var tab = target.closest('[role="tab"]');
      if (tab) select(tab, false);
    });

    stage.addEventListener("keydown", function (event) {
      var target = event.target;
      if (!(target instanceof Element) || target.getAttribute("role") !== "tab") return;
      var index = tabs.indexOf(target);
      var next = null;
      if (event.key === "ArrowRight") next = tabs[(index + 1) % tabs.length];
      else if (event.key === "ArrowLeft") next = tabs[(index - 1 + tabs.length) % tabs.length];
      else if (event.key === "Home") next = tabs[0];
      else if (event.key === "End") next = tabs[tabs.length - 1];
      if (next) {
        event.preventDefault();
        select(next, true);
      }
    });
  }
})();
