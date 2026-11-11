//! 自动更新（思路同 Sparkle：版本清单 + 签名校验 + 用户同意后才安装）。见 docs/updates.md。
//!
//! - 检查：读 `latest.json`（先七牛、再 GitHub，见 `feed`），与当前版本按 semver 比较。
//!   开着「自动检查更新」时，启动后与之后每 24 小时检查一次；失败（离线）不打扰，1 小时后再试。
//!   手动检查（菜单栏「检查更新…」、设置 › 关于、面板里的「检查更新」命令）把结果或失败原因显示在关于页。
//! - 安装：只在用户点「下载并安装」后进行。下载 zip，校验大小与 sha256，解压后校验签名、Team ID 与
//!   Gatekeeper，再由助手脚本在 Helmp 退出后原地替换并重新打开（见 `install`）。
//!   不能原地替换时（开发版、不在「应用程序」里、没有写权限）改为下载 dmg 并在访达中打开。
//! - 演练模式（`HELMP_DRY_RUN=1`）只下载、校验并打印计划，绝不替换任何 App。
//!
//! 状态是进程级的（菜单栏、设置窗口、面板命令共用），界面按 `generation()` 轮询重绘。

pub mod feed;
pub mod install;
pub mod manifest;
pub mod notes;
pub mod version;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, bail};

use crate::i18n::{tr, trf};
use install::{Location, Runner};
use manifest::{AssetKind, Manifest};
use version::Version;

/// 自动检查的间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// 自动检查失败（多半是离线）后多久再试。
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// 启动后多久做第一次自动检查：不和启动时的索引抢资源。
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Busy {
    #[default]
    Idle,
    Checking,
    Downloading {
        done: u64,
        total: u64,
    },
    Verifying,
    /// 新版本已就位，Helmp 即将退出，由助手替换并重新打开。
    Relaunching,
}

/// 更新状态快照。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub busy: Busy,
    /// 最近一次成功检查拿到的清单（不论是否比当前新）。
    pub latest: Option<Arc<Manifest>>,
    /// 清单来自哪个地址。
    pub source: Option<String>,
    pub check_error: Option<String>,
    pub install_error: Option<String>,
    /// 安装后的说明（演练计划、dmg 已打开）。
    pub notice: Option<String>,
    pub checked_at: Option<SystemTime>,
}

impl Snapshot {
    /// 比当前版本新的更新。
    pub fn available(&self) -> Option<&Manifest> {
        self.latest
            .as_deref()
            .filter(|manifest| manifest.version > Version::current())
    }

    /// 这台 Mac 的系统版本不够新时，返回要求的最低版本。
    pub fn unsupported_macos(&self) -> Option<String> {
        let minimum = self.available()?.minimum_macos.clone()?;
        let current = macos_version()?;
        (version::macos_at_least(&current, &minimum) == Some(false)).then_some(minimum)
    }

    /// 关于页 / 通用页的状态行。
    pub fn status_line(&self) -> String {
        match &self.busy {
            Busy::Checking => return tr("正在检查更新…", "Checking for updates…").into(),
            Busy::Downloading { done, total } => {
                let percent = if *total > 0 { done * 100 / total } else { 0 };
                return trf!(
                    "正在下载 {} / {}（{percent}%）",
                    "Downloading {} of {} ({percent}%)",
                    megabytes(*done),
                    megabytes(*total)
                );
            }
            Busy::Verifying => return tr("正在校验签名…", "Verifying the signature…").into(),
            Busy::Relaunching => {
                return tr("即将重新打开 Helmp…", "Relaunching Helmp…").into();
            }
            Busy::Idle => {}
        }
        if let Some(err) = &self.install_error {
            return trf!("安装失败：{err}", "Update failed: {err}");
        }
        if let Some(err) = &self.check_error {
            return trf!("检查失败：{err}", "Couldn't check for updates: {err}");
        }
        match (&self.latest, self.available()) {
            (_, Some(manifest)) => match self.unsupported_macos() {
                Some(minimum) => trf!(
                    "发现 {}，但需要 macOS {minimum} 或更新版本",
                    "Version {} is available but needs macOS {minimum} or later",
                    manifest.version
                ),
                None => trf!("发现新版本 {}", "Version {} is available", manifest.version),
            },
            (Some(_), None) => trf!(
                "已是最新版本（{}）",
                "You're up to date ({})",
                env!("CARGO_PKG_VERSION")
            ),
            (None, None) => tr("尚未检查更新", "Not checked yet").into(),
        }
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

static STATE: Mutex<Option<Snapshot>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static CHECK_REQUESTED: AtomicBool = AtomicBool::new(false);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn snapshot() -> Snapshot {
    STATE
        .lock()
        .map(|state| state.clone().unwrap_or_default())
        .unwrap_or_default()
}

/// 状态每变一次加一：菜单栏、设置窗口据此重绘。
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

fn set(f: impl FnOnce(&mut Snapshot)) {
    if let Ok(mut state) = STATE.lock() {
        f(state.get_or_insert_with(Snapshot::default));
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// 这台 Mac 的系统版本（`sw_vers -productVersion`），查一次后缓存。
pub fn macos_version() -> Option<String> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let out = std::process::Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (out.status.success() && !text.is_empty()).then_some(text)
        })
        .clone()
}

/// 检查一次（阻塞）。前面的地址失败、后面的成功时，把失败原因打到 stderr（不悄悄跳过）。
pub fn check_blocking() -> Result<feed::Fetched> {
    let urls = feed::feed_urls(&crate::config::get());
    let fetched = feed::fetch(&urls)?;
    for (url, err) in &fetched.skipped {
        eprintln!(
            "helmp: {}",
            trf!(
                "更新源 {url} 不可用（{err}），已改用 {}",
                "update feed {url} is unavailable ({err}); used {} instead",
                fetched.source
            )
        );
    }
    Ok(fetched)
}

/// 在后台线程检查。`manual` = 用户点的：失败原因显示在关于页；自动检查失败只记日志。
/// 已经在检查或安装时什么也不做。
pub fn check_in_background(manual: bool) {
    let mut started = false;
    set(|state| {
        if state.busy == Busy::Idle {
            state.busy = Busy::Checking;
            started = true;
        }
    });
    if !started {
        return;
    }
    std::thread::spawn(move || {
        let result = check_blocking();
        set(|state| {
            state.busy = Busy::Idle;
            state.checked_at = Some(SystemTime::now());
            match result {
                Ok(fetched) => {
                    state.latest = Some(Arc::new(fetched.manifest));
                    state.source = Some(fetched.source);
                    state.check_error = None;
                    state.install_error = None;
                }
                Err(err) => {
                    let text = format!("{err:#}").replace('\n', "; ");
                    eprintln!("helmp: 检查更新失败（{}）：{text}", if manual { "手动" } else { "自动" });
                    state.check_error = Some(text);
                }
            }
        });
    });
}

/// 面板命令「检查更新」：动作在后台线程执行、也可能在命令行里执行，只做标记，
/// 由图形界面的轮询任务（`start`）发起检查。
pub fn request_check() {
    CHECK_REQUESTED.store(true, Ordering::SeqCst);
}

/// 安装的结果。
#[derive(Debug)]
pub enum Installed {
    /// 新版本已就位，助手等这个进程退出后替换并重新打开。
    Relaunching { bundle: PathBuf, log: PathBuf },
    /// 无法原地替换：dmg 已下载、校验并打开。
    OpenedDmg { path: PathBuf, reason: String },
    /// 演练：只列出步骤。
    DryRun(Vec<String>),
}

/// 当前运行的 Helmp 的 Team ID：新版本必须与它一致。
fn running_team(runner: &dyn Runner, bundle: &std::path::Path) -> Result<String> {
    install::team_id(runner, bundle)?.with_context(|| {
        tr(
            "当前运行的 Helmp 没有 Developer ID 签名，无法确认更新来自同一开发者，请手动下载安装",
            "This Helmp has no Developer ID signature, so the update's origin can't be checked; please download it manually",
        )
    })
}

/// 安装方式的说明行（计划 / 演练用）。
pub fn plan(manifest: &Manifest, location: &Location) -> Vec<String> {
    let mut lines = vec![trf!(
        "更新 {} → {}",
        "Update {} → {}",
        env!("CARGO_PKG_VERSION"),
        manifest.version
    )];
    let kind = match location {
        Location::Replace { .. } => AssetKind::Zip,
        Location::Manual { reason } => {
            lines.push(trf!("无法原地替换：{reason}", "Can't replace in place: {reason}"));
            AssetKind::Dmg
        }
    };
    match manifest.asset(kind) {
        Some(asset) => lines.push(trf!(
            "下载 {}（{} 字节，sha256 {}），依次尝试：{}",
            "Download {} ({} bytes, sha256 {}), trying in order: {}",
            asset.name,
            asset.size,
            asset.sha256,
            asset.urls.join(", ")
        )),
        None => lines.push(trf!(
            "清单里没有 {} 安装包",
            "The manifest has no {} asset",
            kind.as_str()
        )),
    }
    match location {
        Location::Replace { bundle } => lines.extend(install::replace_steps(bundle, &manifest.version)),
        Location::Manual { .. } => lines.push(tr(
            "保存到「下载」文件夹并在访达中打开，由你把 Helmp 拖进「应用程序」",
            "Save it to Downloads and open it in Finder, so you can drag Helmp to Applications",
        ).to_string()),
    }
    lines
}

/// 下载、校验并安装（阻塞）。`relaunch`：替换后是否重新打开（图形界面是，命令行看情况）。
pub fn install_blocking(
    manifest: &Manifest,
    location: &Location,
    runner: &dyn Runner,
    relaunch: bool,
    progress: &mut dyn FnMut(Busy),
) -> Result<Installed> {
    let dry = crate::effects::dry_run();
    let kind = match location {
        Location::Replace { .. } => AssetKind::Zip,
        Location::Manual { .. } => AssetKind::Dmg,
    };
    let asset = manifest.asset(kind).with_context(|| {
        trf!(
            "更新清单里没有 {} 安装包",
            "The update manifest has no {} asset",
            kind.as_str()
        )
    })?;
    // 在下载前确认能拿到当前 App 的 Team ID：拿不到就不必下载了。
    let team = match (location, dry) {
        (Location::Replace { bundle }, false) => Some(running_team(runner, bundle)?),
        _ => None,
    };
    progress(Busy::Downloading { done: 0, total: asset.size });
    let (bytes, url) = feed::download(asset, &mut |done, total| {
        progress(Busy::Downloading { done, total })
    })?;
    progress(Busy::Verifying);
    let verified = trf!(
        "已下载 {}（{url}）：大小 {} 字节与 sha256 {} 校验通过",
        "Downloaded {} ({url}): size {} bytes and sha256 {} verified",
        asset.name,
        asset.size,
        asset.sha256
    );
    if dry {
        let mut lines = vec![format!("[dry-run] {verified}")];
        lines.extend(
            plan(manifest, location)
                .into_iter()
                .filter(|line| !line.starts_with(tr("下载 ", "Download ")))
                .map(|line| format!("[dry-run] {line}")),
        );
        lines.push(
            tr(
                "[dry-run] 演练模式：没有解压、没有替换任何 App",
                "[dry-run] Dry run: nothing was unpacked and no app was replaced",
            )
            .to_string(),
        );
        return Ok(Installed::DryRun(lines));
    }
    match location {
        Location::Replace { bundle } => {
            let team = team.unwrap_or_default();
            let work = std::env::temp_dir().join(format!("helmp-update-{}", std::process::id()));
            std::fs::create_dir_all(&work)
                .with_context(|| trf!("无法创建 {}", "Couldn't create {}", work.display()))?;
            let zip = work.join(&asset.name);
            std::fs::write(&zip, &bytes)
                .with_context(|| trf!("无法写入 {}", "Couldn't write {}", zip.display()))?;
            let staged = install::stage(&zip, bundle, &manifest.version, runner, &team);
            let _ = std::fs::remove_file(&zip);
            let staged = staged?;
            let log = install::spawn_helper(std::process::id(), bundle, &staged, relaunch)?;
            Ok(Installed::Relaunching {
                bundle: bundle.clone(),
                log,
            })
        }
        Location::Manual { reason } => {
            let downloads = crate::paths::home_dir()?.join("Downloads");
            std::fs::create_dir_all(&downloads)
                .with_context(|| trf!("无法创建 {}", "Couldn't create {}", downloads.display()))?;
            let path = downloads.join(&asset.name);
            std::fs::write(&path, &bytes)
                .with_context(|| trf!("无法写入 {}", "Couldn't write {}", path.display()))?;
            let status = std::process::Command::new("/usr/bin/open")
                .arg(&path)
                .status()
                .context("open")?;
            if !status.success() {
                bail!(trf!("无法打开 {}", "Couldn't open {}", path.display()));
            }
            Ok(Installed::OpenedDmg {
                path,
                reason: reason.clone(),
            })
        }
    }
}

/// 用户点了「下载并安装」：在后台线程安装最近检查到的更新。成功原地替换后请求退出（由 `start` 执行）。
pub fn install_in_background() {
    let mut manifest = None;
    set(|state| {
        if state.busy == Busy::Idle
            && let Some(available) = state.available()
        {
            manifest = Some(available.clone());
            state.busy = Busy::Downloading { done: 0, total: 0 };
            state.install_error = None;
            state.notice = None;
        }
    });
    let Some(manifest) = manifest else {
        return;
    };
    std::thread::spawn(move || {
        let location = install::current_location();
        let mut last_report = Instant::now();
        let result = install_blocking(
            &manifest,
            &location,
            &install::SystemRunner,
            true,
            &mut |busy| {
                // 进度最多每 100 ms 刷新一次，避免界面无谓重绘。
                let is_progress = matches!(busy, Busy::Downloading { .. });
                if !is_progress || last_report.elapsed() >= Duration::from_millis(100) {
                    last_report = Instant::now();
                    set(|state| state.busy = busy);
                }
            },
        );
        set(|state| {
            state.busy = Busy::Idle;
            match result {
                Ok(Installed::Relaunching { bundle, log }) => {
                    crate::trace::log(format!(
                        "update: 新版本已就位，退出后替换 {}（日志 {}）",
                        bundle.display(),
                        log.display()
                    ));
                    state.busy = Busy::Relaunching;
                    QUIT_REQUESTED.store(true, Ordering::SeqCst);
                }
                Ok(Installed::OpenedDmg { path, reason }) => {
                    state.notice = Some(trf!(
                        "{reason}。已下载并打开 {}，请把 Helmp 拖进「应用程序」替换旧版本。",
                        "{reason}. Downloaded and opened {}; drag Helmp to Applications to replace the old version.",
                        path.display()
                    ));
                }
                Ok(Installed::DryRun(lines)) => {
                    for line in &lines {
                        println!("{line}");
                    }
                    state.notice = Some(lines.join("\n"));
                }
                Err(err) => {
                    let text = format!("{err:#}").replace('\n', "; ");
                    eprintln!("helmp: 安装更新失败：{text}");
                    state.install_error = Some(text);
                }
            }
        });
    });
}

/// 图形界面：自动检查的定时、面板命令的检查请求、安装就绪后的退出。启动时调用一次。
pub fn start(cx: &mut gpui_kit::App) {
    use gpui_kit::AsyncApp;
    let started = Instant::now();
    cx.spawn(async move |cx: &mut AsyncApp| {
        // 下一次自动检查的时间；开关关着时到点也不查，打开后立即补一次。
        let mut next_auto = started + FIRST_CHECK_DELAY;
        // 一次自动检查正在进行：结束后按成败安排下一次（成功 24 小时，失败 1 小时）。
        let mut auto_in_flight = false;
        loop {
            if CHECK_REQUESTED.swap(false, Ordering::SeqCst) {
                check_in_background(true);
            }
            if QUIT_REQUESTED.swap(false, Ordering::SeqCst) {
                // 与菜单「退出」相同：MCP 服务器子进程一起结束。助手在等这个进程退出。
                crate::providers::mcp::manager::global().stop_all();
                cx.update(|cx| cx.quit());
                break;
            }
            let now = Instant::now();
            if auto_in_flight {
                let state = snapshot();
                if state.busy != Busy::Checking {
                    auto_in_flight = false;
                    let wait = if state.check_error.is_some() {
                        RETRY_INTERVAL
                    } else {
                        CHECK_INTERVAL
                    };
                    next_auto = now + wait;
                }
            } else if now >= next_auto && crate::config::get().general.auto_check_updates {
                if snapshot().busy == Busy::Idle {
                    check_in_background(false);
                    auto_in_flight = true;
                } else {
                    // 正在手动检查或安装：稍后再说。
                    next_auto = now + RETRY_INTERVAL;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
        }
    })
    .detach();
}

// ---------------------------------------------------------------- 命令行

/// `helmp update check | install [--yes]`。
pub fn cli(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("check") => cli_check(),
        Some("install") => cli_install(args[1..].iter().any(|a| a == "--yes")),
        _ => bail!(tr(
            "用法：helmp update check | helmp update install [--yes]",
            "Usage: helmp update check | helmp update install [--yes]"
        )),
    }
}

fn cli_check() -> Result<()> {
    let fetched = check_blocking().map_err(|err| {
        anyhow::anyhow!(trf!(
            "检查更新失败：\n{err:#}",
            "Couldn't check for updates:\n{err:#}"
        ))
    })?;
    let current = Version::current();
    let latest = &fetched.manifest.version;
    let status = if latest > &current {
        "update-available"
    } else {
        "up-to-date"
    };
    println!("current: {current}");
    println!("latest: {latest}");
    println!("status: {status}");
    println!("feed: {}", fetched.source);
    if latest > &current
        && let Some(notes) = fetched.manifest.notes()
    {
        println!("notes: {}", notes.replace('\n', "\n       "));
    }
    Ok(())
}

/// 另一个 Helmp（图形界面）正在运行吗？命令行替换它正在用的 App 会出乱子。
fn gui_running() -> bool {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;
    let me = std::process::id() as i32;
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        install::BUNDLE_ID,
    ))
    .iter()
    .any(|app| app.processIdentifier() != me)
}

fn cli_install(yes: bool) -> Result<()> {
    let fetched = check_blocking().map_err(|err| {
        anyhow::anyhow!(trf!(
            "检查更新失败：\n{err:#}",
            "Couldn't check for updates:\n{err:#}"
        ))
    })?;
    let manifest = fetched.manifest;
    if manifest.version <= Version::current() {
        println!(
            "{}",
            trf!(
                "已是最新版本（{}，更新源上是 {}）",
                "Already up to date ({}; the feed has {})",
                env!("CARGO_PKG_VERSION"),
                manifest.version
            )
        );
        return Ok(());
    }
    let location = install::current_location();
    if !yes {
        for line in plan(&manifest, &location) {
            println!("{line}");
        }
        bail!(tr(
            "还没有安装：确认无误后加 --yes 再运行（替用户操作的 agent 必须先征得用户同意）",
            "Nothing installed yet: run again with --yes once you've confirmed (agents must ask the user first)"
        ));
    }
    if !crate::effects::dry_run() && matches!(location, Location::Replace { .. }) && gui_running() {
        bail!(tr(
            "Helmp 正在运行：请用菜单栏的「更新到 …」安装，或先退出 Helmp 再运行这个命令",
            "Helmp is running: install from the menu bar's \"Update to …\" item, or quit Helmp first"
        ));
    }
    let result = install_blocking(&manifest, &location, &install::SystemRunner, false, &mut |_| {})?;
    match result {
        Installed::DryRun(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Installed::Relaunching { bundle, log } => println!(
            "{}",
            trf!(
                "已校验 {}，将在本命令退出后替换 {}（日志：{}）",
                "Verified {}; {} will be replaced once this command exits (log: {})",
                manifest.version,
                bundle.display(),
                log.display()
            )
        ),
        Installed::OpenedDmg { path, reason } => println!(
            "{}",
            trf!(
                "{reason}。已下载并打开 {}",
                "{reason}. Downloaded and opened {}",
                path.display()
            )
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> Arc<Manifest> {
        Arc::new(manifest::parse(&manifest::tests::sample(version)).unwrap())
    }

    #[test]
    fn status_line_reflects_check_results() {
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let mut state = Snapshot::default();
            assert_eq!(state.status_line(), "Not checked yet");
            state.latest = Some(manifest("99.0.0"));
            assert!(state.available().is_some());
            assert!(state.status_line().contains("99.0.0"), "{}", state.status_line());
            state.latest = Some(manifest(env!("CARGO_PKG_VERSION")));
            assert!(state.available().is_none());
            assert!(state.status_line().starts_with("You're up to date"));
            state.latest = Some(manifest("0.0.1"));
            assert!(state.available().is_none(), "旧版本不算更新");
            state.check_error = Some("offline".into());
            assert_eq!(state.status_line(), "Couldn't check for updates: offline");
            state.busy = Busy::Downloading { done: 5_000_000, total: 10_000_000 };
            assert!(state.status_line().contains("50%"));
        });
    }

    #[test]
    fn plan_uses_zip_in_place_and_dmg_otherwise() {
        let manifest = manifest("99.0.0");
        let replace = plan(
            &manifest,
            &Location::Replace { bundle: PathBuf::from("/Applications/Helmp.app") },
        )
        .join("\n");
        assert!(replace.contains("Helmp-99.0.0.zip") && replace.contains("/Applications/Helmp.app"), "{replace}");
        assert!(replace.contains("codesign --verify --deep --strict"));
        let manual = plan(&manifest, &Location::Manual { reason: "dev build".into() }).join("\n");
        assert!(manual.contains("Helmp-99.0.0.dmg") && manual.contains("dev build"), "{manual}");
        assert!(!manual.contains(".zip"));
    }
}
