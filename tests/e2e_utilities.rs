//! 实用工具端到端测试：单位与汇率、时间与时间戳、开发工具、Emoji、颜色。
//! 真实的 helmp 可执行文件 + 隔离环境（见 tests/support）：不碰剪贴板、不联真实网络（汇率和公网 IP 都指向
//! 本地 MockServer 或一个关掉的端口），时间用 HELMP_NOW 固定，时区用 TZ 固定。

mod support;

use support::{Helmp, MockServer};

const ECB_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01" xmlns="http://www.ecb.int/vocabulary/2002-08-01/eurofxref">
	<gesmes:subject>Reference rates</gesmes:subject>
	<Cube>
		<Cube time='2026-11-09'>
			<Cube currency='USD' rate='1.1000'/>
			<Cube currency='JPY' rate='165.00'/>
			<Cube currency='CNY' rate='7.7000'/>
			<Cube currency='HKD' rate='8.5000'/>
		</Cube>
	</Cube>
</gesmes:Envelope>"#;

/// 一个当前没人监听的本地端口：模拟离线。
fn dead_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}/eurofxref-daily.xml")
}

fn ecb_server() -> MockServer {
    MockServer::start(|_| (200, ECB_XML.to_string()))
}

// ---------------------------------------------------------------- 单位

#[test]
fn units_convert_natural_english_and_chinese_input() {
    // 用户怎么顺手怎么写：英文缩写、无空格、中文单位都要直接出答案并排第一，回车复制纯数值。
    let h = Helmp::new();
    let rows = h.query("10 km to mi");
    let first = rows.first().expect("有结果");
    assert_eq!(
        (first.id.as_str(), first.title.as_str()),
        ("calculator:unit:mi", "6.21371 mi")
    );
    assert_eq!(
        h.run("calculator:unit:mi", "10 km to mi", &[]).trim(),
        "[dry-run] copy 6.21371"
    );
    let out = h.run(
        "calculator:unit:mi",
        "10 km to mi",
        &["--action", "Copy with Unit"],
    );
    assert_eq!(out.trim(), "[dry-run] copy 6.21371 mi");

    let jin = h.find("3斤", |r| r.id == "calculator:unit:kg");
    assert_eq!(jin.title, "1.5 kg");
    assert_eq!(jin.subtitle, "3 斤 =");
    assert_eq!(
        h.find("100f in c", |r| r.id == "calculator:unit:c").title,
        "37.7778 °C"
    );
    assert_eq!(
        h.find("10公里等于多少英里", |r| r.id
            == "calculator:unit:mi")
            .title,
        "6.21371 mi"
    );
    assert_eq!(
        h.find("2亩是多少平方米", |r| r.id == "calculator:unit:m2")
            .title,
        "1333.33 m²"
    );
    // 1 GB 是 1000 MB（访达的算法），另给一行 1024 进制，免得用户拿错。
    let gb = h.query("1 GB = ? MB");
    assert_eq!(gb.first().map(|r| r.title.as_str()), Some("1000 MB"));
    assert!(
        gb.iter()
            .any(|r| r.id == "calculator:unit:mb-1024" && r.title == "1024 MB"),
        "{gb:#?}"
    );
}

#[test]
fn units_never_hijack_plain_searches_or_break_the_calculator() {
    // 「5g」「2 days」这类搜索词不能冒出换算行；原来的算式、日期排除规则保持不变。
    let h = Helmp::new();
    for query in ["5g", "2 days", "1password"] {
        let rows = h.query(query);
        assert!(
            !rows.iter().any(|r| r.id.starts_with("calculator:")),
            "{query}: {rows:#?}"
        );
    }
    assert_eq!(h.find("2+3*4", |r| r.id == "calculator:result").title, "14");
    assert!(
        !h.query("2026-11-09")
            .iter()
            .any(|r| r.id.starts_with("calculator:"))
    );
}

// ---------------------------------------------------------------- 汇率

#[test]
fn currency_converts_with_ecb_rates_and_caches_them() {
    // 汇率来自 ECB：第一次联网取到并写缓存，结果写明汇率日期；缓存新鲜时不再请求（每天最多一次）。
    let server = ecb_server();
    let url = format!("{}/eurofxref-daily.xml", server.url);
    let h = Helmp::new().env("HELMP_FX_URL", url);
    let row = h.find("100 usd to cny", |r| r.id == "calculator:fx:CNY");
    assert_eq!(row.title, "700.00 CNY");
    assert!(
        row.subtitle.contains("Rates as of 2026-11-09 (ECB)"),
        "{row:?}"
    );
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].path.ends_with("eurofxref-daily.xml"));
    let cache = std::fs::read_to_string(h.data_file("fx.json")).expect("写了汇率缓存");
    assert!(cache.contains("2026-11-09"), "{cache}");

    // 中文写法、没写目标币种：列出常用币种。
    let rows = h.query("100美元");
    let titles: Vec<_> = rows
        .iter()
        .filter(|r| r.id.starts_with("calculator:fx:"))
        .map(|r| r.title.as_str())
        .collect();
    assert_eq!(
        titles,
        ["700.00 CNY", "90.91 EUR", "15000.00 JPY", "772.73 HKD"]
    );
    assert_eq!(
        h.find("100 刀", |r| r.id == "calculator:fx:CNY").title,
        "700.00 CNY"
    );
    assert_eq!(server.requests().len(), 1, "缓存不到一天，不该再请求");
    assert_eq!(
        h.run("calculator:fx:CNY", "100 usd to cny", &[]).trim(),
        "[dry-run] copy 700.00"
    );
}

#[test]
fn currency_works_offline_from_an_old_cache() {
    // 离线时用缓存照样能换算，但要让用户知道汇率是哪天的、这次没更新成功。
    let h = Helmp::new().env("HELMP_FX_URL", dead_url());
    h.write(
        "data/fx.json",
        r#"{"date":"2026-11-01","fetched_at":0,"source":"ECB","rates":{"EUR":1.0,"USD":1.0,"CNY":7.0,"JPY":150.0,"HKD":7.8}}"#,
    );
    let row = h.find("¥700 to usd", |r| r.id == "calculator:fx:USD");
    assert_eq!(row.title, "100.00 USD");
    assert!(
        row.subtitle.contains("Rates as of 2026-11-01 (ECB)"),
        "{row:?}"
    );
    assert!(
        row.subtitle.contains("update failed, using cache"),
        "{row:?}"
    );
}

#[test]
fn currency_without_cache_or_network_says_so() {
    // 第一次用又没网：不能静默不出结果，要明确说没有汇率数据和原因，回车重试也要报错而不是假装成功。
    let h = Helmp::new().env("HELMP_FX_URL", dead_url());
    let row = h.find("100 usd to cny", |r| r.id == "calculator:fx-unavailable");
    assert!(
        row.subtitle.contains("No exchange-rate data yet"),
        "{row:?}"
    );
    assert!(
        row.subtitle.contains("Couldn't fetch exchange rates"),
        "{row:?}"
    );
    let out = h.run("calculator:fx-unavailable", "100 usd to cny", &[]);
    assert!(out.contains("Couldn’t update exchange rates"), "{out}");

    // 演练模式下没指定地址时根本不联网（测试永远碰不到 ECB），同样明确提示。
    let offline = Helmp::new();
    let row = offline.find("汇率", |r| r.id == "calculator:fx-unavailable");
    assert!(
        row.subtitle.contains("No exchange-rate data yet"),
        "{row:?}"
    );
}

// ---------------------------------------------------------------- 时间

fn timed() -> Helmp {
    Helmp::new()
        .env("HELMP_NOW", "2026-11-10T12:00:00Z")
        .env("TZ", "Asia/Shanghai")
}

#[test]
fn timestamps_convert_to_local_utc_and_iso() {
    // 日志里的时间戳：秒和毫秒都要认，给出本地时间、UTC、ISO，回车复制。
    let h = timed();
    let rows = h.query("1731234567");
    let get = |id: &str| {
        rows.iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("{id}: {rows:#?}"))
            .title
            .clone()
    };
    assert_eq!(get("time:local"), "2024-11-10 18:29:27 Sun");
    assert_eq!(get("time:utc"), "2024-11-10 10:29:27 UTC");
    assert_eq!(get("time:iso"), "2024-11-10T18:29:27+08:00");
    assert_eq!(get("time:relative"), "2 years ago");
    assert_eq!(
        rows.first().map(|r| r.id.as_str()),
        Some("time:local"),
        "时间戳结果排第一"
    );
    let ms = h.find("1731234567890", |r| r.id == "time:utc");
    assert_eq!(ms.title, "2024-11-10 10:29:27 UTC");
    assert_eq!(
        h.run("time:iso", "1731234567", &[]).trim(),
        "[dry-run] copy 2024-11-10T18:29:27+08:00"
    );
}

#[test]
fn now_zones_and_relative_dates_follow_frozen_now() {
    // HELMP_NOW 固定了「现在」：now、城市时间、跨时区换算（含冬令时）、相对时间都要按它算。
    let h = timed();
    assert_eq!(h.find("now", |r| r.id == "time:unix").title, "1794312000");
    assert_eq!(
        h.find("时间", |r| r.id == "time:local").title,
        "2026-11-10 20:00:00 Tue"
    );
    let tokyo = h.find("东京时间", |r| r.id == "time:zone:Asia/Tokyo");
    assert_eq!(tokyo.title, "2026-11-10 21:00:00 Tue");
    assert_eq!(
        h.find("time tokyo", |r| r.id == "time:zone:Asia/Tokyo")
            .title,
        tokyo.title
    );
    // 北京 15:00 = UTC 07:00 = 纽约 02:00（11 月已是 EST）。
    let rows = h.query("3pm 北京 in 纽约");
    assert_eq!(
        rows.first().map(|r| r.title.as_str()),
        Some("2026-11-10 02:00:00 Tue"),
        "{rows:#?}"
    );
    let la = h.find("2026-11-10 10:00 pst to beijing", |r| {
        r.id == "time:zone:Asia/Shanghai"
    });
    assert_eq!(la.title, "2026-11-11 02:00:00 Wed");
    assert_eq!(
        h.find("in 3 days", |r| r.id == "time:local").title,
        "2026-11-13 20:00:00 Fri"
    );
    assert_eq!(
        h.find("3天前", |r| r.id == "time:local").title,
        "2026-11-07 20:00:00 Sat"
    );
    assert_eq!(
        h.find("2026-11-10 12:00 to unix", |r| r.id == "time:unix")
            .title,
        "1794283200"
    );
}

// ---------------------------------------------------------------- 开发工具

#[test]
fn dev_tools_transform_text_and_copy_results() {
    // 每个工具的输出用固定输入校验：错一个字符这些结果就没用了。回车复制的必须是结果本身。
    let h = Helmp::new();
    let md5 = h.find("md5 hello", |r| r.id == "devtools:hash:md5");
    assert_eq!(md5.title, "5d41402abc4b2a76b9719d911017c592");
    assert_eq!(
        h.run("devtools:hash:md5", "md5 hello", &[]).trim(),
        "[dry-run] copy 5d41402abc4b2a76b9719d911017c592"
    );
    assert_eq!(
        h.find("sha256 hello", |r| r.id == "devtools:hash:sha256")
            .title,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(
        h.find("base64 你好", |r| r.id == "devtools:base64:encode")
            .title,
        "5L2g5aW9"
    );
    assert_eq!(
        h.find("base64 5L2g5aW9", |r| r.id == "devtools:base64:decode")
            .title,
        "你好"
    );
    assert_eq!(
        h.find("url a b&c", |r| r.id == "devtools:url:encode").title,
        "a%20b%26c"
    );
    assert_eq!(
        h.find("url a%20b", |r| r.id == "devtools:url:decode").title,
        "a b"
    );
    assert_eq!(
        h.find("case hello world", |r| r.id == "devtools:case:camel")
            .title,
        "helloWorld"
    );
    assert_eq!(
        h.find("snake helloWorld", |r| r.id == "devtools:case:snake")
            .title,
        "hello_world"
    );
    assert_eq!(h.find("0xff", |r| r.id == "devtools:base:dec").title, "255");
    assert_eq!(
        h.find("hex 255", |r| r.id == "devtools:base:hex").title,
        "0xff"
    );
    assert_eq!(
        h.find("0b1010", |r| r.id == "devtools:base:oct").title,
        "0o12"
    );

    // JSON：格式化保持键的顺序，回车复制完整的多行结果；无效时说清楚位置。
    let out = h.run("devtools:json:pretty", r#"json {"b":1,"a":[1,2]}"#, &[]);
    assert_eq!(
        out.trim_end(),
        "[dry-run] copy {\n  \"b\": 1,\n  \"a\": [\n    1,\n    2\n  ]\n}"
    );
    let bad = h.find(r#"json {"a":}"#, |r| r.id == "devtools:json:error");
    assert!(bad.title.contains("line 1, column 6"), "{bad:?}");

    // JWT：不验签，直接看载荷。
    let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    let payload = h.run("devtools:jwt:payload", token, &[]);
    assert!(payload.contains("\"name\": \"John Doe\""), "{payload}");
    assert_eq!(
        h.find(token, |r| r.id == "devtools:jwt:iat").title,
        "2018-01-18 01:30:22 UTC"
    );
}

#[test]
fn dev_tools_generate_values_in_the_right_shape() {
    // 生成类结果每次不同，只校验格式：UUID 版本位、密码长度和字符集、假文长度。
    let h = Helmp::new();
    let rows = h.query("uuid");
    let v4 = rows
        .iter()
        .find(|r| r.id == "devtools:uuid:v4")
        .expect("v4");
    let v7 = rows
        .iter()
        .find(|r| r.id == "devtools:uuid:v7")
        .expect("v7");
    for (row, version) in [(v4, '4'), (v7, '7')] {
        assert_eq!(row.title.len(), 36, "{row:?}");
        assert_eq!(row.title.chars().nth(14), Some(version), "{row:?}");
    }
    let upper = h.find("uuid upper nodash", |r| r.id == "devtools:uuid:v4");
    assert!(
        upper.title.len() == 32 && !upper.title.chars().any(|c| c.is_ascii_lowercase()),
        "{upper:?}"
    );
    let pwd = h.find("pwd 24 nosym", |r| r.id == "devtools:password:alnum");
    assert_eq!(pwd.title.chars().count(), 24);
    assert!(
        pwd.title.chars().all(|c| c.is_ascii_alphanumeric()),
        "{pwd:?}"
    );
    assert!(pwd.subtitle.contains("24 chars"), "{pwd:?}");
    assert_eq!(
        h.find("lorem 5", |r| r.id == "devtools:lorem:latin").title,
        "Lorem ipsum dolor sit amet."
    );
    // 中文名和拼音首字母也能找到工具，回车进入范围并带上命令。
    let entry = h.find("jzzh", |r| r.id == "devtools:tool:base");
    assert_eq!(
        h.run(&entry.id, "jzzh", &[]).trim(),
        "enter scope devtools: hex"
    );
    // 范围里输入任意文字：列出它的各种转换。
    let scoped = h.query("dev Hello World");
    for id in [
        "devtools:base64:encode",
        "devtools:hash:md5",
        "devtools:case:kebab",
    ] {
        assert!(scoped.iter().any(|r| r.id == id), "{id}: {scoped:#?}");
    }
}

#[test]
fn public_ip_is_fetched_only_on_return_from_the_configured_url() {
    // 本机 IP 不联网；公网 IP 只在回车时请求配置的地址（这里是本地假服务），返回的必须是 IP。
    let server = MockServer::start(|_| (200, "203.0.113.7\n".to_string()));
    let h = Helmp::new().env("HELMP_PUBLIC_IP_URL", server.url.clone());
    let row = h.find("ip", |r| r.id == "devtools:ip:public");
    assert!(row.subtitle.contains(&server.url), "{row:?}");
    assert!(server.requests().is_empty(), "查询时不能联网");
    let out = h.run("devtools:ip:public", "ip", &[]);
    assert!(out.contains("203.0.113.7"), "{out}");
    assert_eq!(server.requests().len(), 1);
    let copied = h.run("devtools:ip:public", "ip", &["--then", "Copy"]);
    assert!(copied.contains("[dry-run] copy 203.0.113.7"), "{copied}");

    // 没指定测试地址时，演练模式只报告将要访问哪里。
    let dry = Helmp::new().run("devtools:ip:public", "ip", &[]);
    assert_eq!(dry.trim(), "[dry-run] fetch https://api.ipify.org");
}

// ---------------------------------------------------------------- Emoji

#[test]
fn emoji_search_in_chinese_pinyin_and_english_then_paste() {
    // 中文、拼音、英文都能搜到同一个 emoji；回车粘贴，⌘C 只复制。
    let h = Helmp::new();
    let cat = h.find("emoji 猫", |r| r.title.starts_with('🐱'));
    assert_eq!(cat.id, "emoji:1f431");
    assert_eq!(
        h.find("emoji cat face", |r| r.id == "emoji:1f431").title,
        "🐱  cat face"
    );
    assert!(
        h.query(":xiao")
            .iter()
            .take(10)
            .any(|r| r.title.starts_with('😄')),
        "拼音"
    );
    assert!(
        h.query("emoji smile")
            .iter()
            .take(8)
            .any(|r| r.title.starts_with('😄'))
    );
    assert_eq!(
        h.find("符号 约等于", |r| r.title.starts_with('≈')).badge,
        "Symbols"
    );
    assert_eq!(
        h.run("emoji:1f431", "emoji 猫", &[]).trim(),
        "[dry-run] paste 🐱"
    );
    assert_eq!(
        h.run("emoji:1f431", "emoji 猫", &["--action", "Copy"])
            .trim(),
        "[dry-run] copy 🐱"
    );
    assert_eq!(
        h.run(
            "emoji:1f44d",
            "emoji thumbs up",
            &["--action", "Paste 👍🏽 (medium skin tone)"]
        )
        .trim(),
        "[dry-run] paste 👍🏽"
    );
    // 用过的排在范围首页最前面。
    let home = h.query("emoji ");
    assert_eq!(
        home.first().map(|r| r.id.as_str()),
        Some("emoji:1f44d"),
        "{:#?}",
        home.get(..3)
    );
    assert_eq!(home.get(1).map(|r| r.id.as_str()), Some("emoji:1f431"));
}

// ---------------------------------------------------------------- 颜色

#[test]
fn typed_colors_convert_to_every_format() {
    // 输入任意一种写法都出全部写法，回车复制那一行；色块图片写在数据目录里。
    let h = Helmp::new();
    let rows = h.query("#ff8800");
    let titles: Vec<_> = rows
        .iter()
        .filter(|r| r.id.starts_with("color:"))
        .map(|r| r.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "#ff8800",
            "rgb(255, 136, 0)",
            "hsl(32, 100%, 50%)",
            "oklch(0.7442 0.1812 56.46)",
            "Color(red: 1.000, green: 0.533, blue: 0.000)",
            "NSColor(srgbRed: 1.000, green: 0.533, blue: 0.000, alpha: 1.000)",
        ]
    );
    assert_eq!(
        h.find("hsl(32, 100%, 50%)", |r| r.id == "color:hex").title,
        "#ff8800"
    );
    // rgb(…) 在计算器眼里是「未知函数」，不能冒出一条错误行压在颜色结果上面。
    let rgb = h.query("rgb(255, 136, 0)");
    assert_eq!(
        rgb.first().map(|r| r.id.as_str()),
        Some("color:hex"),
        "{rgb:#?}"
    );
    assert_eq!(
        h.run("color:rgb", "#ff8800", &[]).trim(),
        "[dry-run] copy rgb(255, 136, 0)"
    );
    assert!(h.data_file("colors/ff8800ff.png").exists(), "色块图片");
}

#[test]
fn pick_color_is_dry_run_gated_and_shows_every_format() {
    // 取色会打开系统取色器：演练模式下只回报；用 HELMP_COLOR_FIXTURE 模拟取到的颜色，检查结果详情和复制。
    let h = Helmp::new();
    h.find("qs", |r| r.id == "color:pick");
    assert_eq!(
        h.run("color:pick", "取色", &[]).trim(),
        "[dry-run] pick-color"
    );

    let picked = Helmp::new().env("HELMP_COLOR_FIXTURE", "#3366ff");
    let out = picked.run("color:pick", "pick color", &[]);
    assert!(out.contains("#3366ff"), "{out}");
    assert!(
        out.contains("SwiftUI: Color(red: 0.200, green: 0.400, blue: 1.000)"),
        "{out}"
    );
    assert!(out.contains("Actions: Copy HEX / Copy RGB"), "{out}");
    let copied = picked.run("color:pick", "pick color", &["--then", "Copy RGB"]);
    assert!(
        copied.contains("[dry-run] copy rgb(51, 102, 255)"),
        "{copied}"
    );
}
