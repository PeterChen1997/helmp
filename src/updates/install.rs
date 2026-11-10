//! 安装更新：解压 → 校验签名 → 交给一个小助手脚本，等 Helmp 退出后原地替换并重新打开。
//!
//! 校验（任何一项不过都拒绝安装，并删掉解压出来的东西）：
//! 1. 下载内容的大小与 sha256 与清单一致（见 `feed::verify_payload`）。
//! 2. 解压出的 `Helmp.app` 的 bundle id 是 `dev.helmp.launcher`，版本号与清单一致（防止回滚到旧版本）。
//! 3. `codesign --verify --deep --strict` 通过。
//! 4. 签名的 Team ID 与正在运行的 Helmp 相同：被篡改的更新源也装不进别人签名的代码。
//! 5. `spctl --assess --type execute` 通过（Developer ID 签名且已公证）。
//!
//! 替换：新版本先解压到 App 所在目录下的隐藏目录（同一卷，替换只是两次改名），助手脚本等 Helmp
//! 退出后把旧版本改名为 `*.old-<pid>`、新版本放到原路径、删掉旧版本，再重新打开。路径不变，
//! Developer ID 的指定要求也不变，所以辅助功能等授权和登录项都继续有效。
//!
//! 只有在 `/Applications` 或 `~/Applications`（含子目录）里、且所在目录可写时才原地替换；
//! 否则（从 target/ 直接运行的开发版、被系统随机化路径运行、没有写权限）改为下载 dmg 并在访达中打开。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::version::Version;
use crate::i18n::{tr, trf};

pub const BUNDLE_ID: &str = "dev.helmp.launcher";
/// 解压目录的前缀；助手脚本只会删除这个前缀的目录。
const STAGE_PREFIX: &str = ".helmp-update-";

/// 外部命令的输出。
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 执行 `codesign` / `spctl`：测试里换成假的，验证校验逻辑而不依赖真实签名。
pub trait Runner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput>;
}

pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
        let out = std::process::Command::new(program)
            .args(args)
            .output()
            .with_context(|| trf!("无法运行 {program}", "Couldn't run {program}"))?;
        Ok(CmdOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

fn path_arg(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| trf!("路径不是 UTF-8：{}", "Path isn't UTF-8: {}", path.display()))
}

/// 签名的 Team ID（`codesign -dv` 输出里的 `TeamIdentifier=`）；没有签名或 ad-hoc 签名时为 None。
pub fn team_id(runner: &dyn Runner, app: &Path) -> Result<Option<String>> {
    let out = runner.run("/usr/bin/codesign", &["-dv", "--verbose=2", path_arg(app)?])?;
    // codesign -d 把信息写到 stderr。
    let text = format!("{}\n{}", out.stderr, out.stdout);
    Ok(text
        .lines()
        .find_map(|line| line.trim().strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|id| !id.is_empty() && *id != "not set")
        .map(str::to_string))
}

fn short_reason(out: &CmdOutput) -> String {
    let text = if out.stderr.trim().is_empty() {
        out.stdout.trim()
    } else {
        out.stderr.trim()
    };
    text.lines().take(3).collect::<Vec<_>>().join(" / ")
}

/// 读 App 的 Info.plist：(bundle id, 版本号)。
fn bundle_info(app: &Path) -> Result<(String, String)> {
    let plist_path = app.join("Contents/Info.plist");
    let value = plist::Value::from_file(&plist_path)
        .with_context(|| trf!("读不了 {}", "Couldn't read {}", plist_path.display()))?;
    let dict = value
        .as_dictionary()
        .with_context(|| trf!("{} 不是字典", "{} isn't a dictionary", plist_path.display()))?;
    let get = |key: &str| {
        dict.get(key)
            .and_then(plist::Value::as_string)
            .unwrap_or_default()
            .to_string()
    };
    Ok((get("CFBundleIdentifier"), get("CFBundleShortVersionString")))
}

/// 校验解压出来的新版本（见模块说明 2–5）。
pub fn verify_app(
    runner: &dyn Runner,
    app: &Path,
    expected_team: &str,
    version: &Version,
) -> Result<()> {
    let (bundle_id, bundle_version) = bundle_info(app)?;
    if bundle_id != BUNDLE_ID {
        bail!(trf!(
            "安装包里的 App 不是 Helmp（bundle id 为 {bundle_id:?}），已拒绝安装",
            "The app in the update isn't Helmp (bundle id {bundle_id:?}); refusing to install"
        ));
    }
    if Version::parse(&bundle_version).ok().as_ref() != Some(version) {
        bail!(trf!(
            "安装包里的版本是 {bundle_version:?}，与更新清单的 {version} 不一致，已拒绝安装",
            "The update contains version {bundle_version:?}, not {version} as the manifest says; refusing to install"
        ));
    }
    let app_arg = path_arg(app)?;
    let out = runner.run(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", "--verbose=2", app_arg],
    )?;
    if !out.success {
        bail!(trf!(
            "新版本的代码签名无效（{}），已拒绝安装",
            "The new version's code signature is invalid ({}); refusing to install",
            short_reason(&out)
        ));
    }
    match team_id(runner, app)? {
        Some(team) if team == expected_team => {}
        Some(team) => bail!(trf!(
            "新版本由另一个开发者签名（Team ID {team}，当前 Helmp 是 {expected_team}），已拒绝安装",
            "The new version is signed by a different developer (Team ID {team}, this Helmp is {expected_team}); refusing to install"
        )),
        None => bail!(tr(
            "新版本没有 Developer ID 签名，已拒绝安装",
            "The new version has no Developer ID signature; refusing to install"
        )),
    }
    let out = runner.run(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute", "--verbose", app_arg],
    )?;
    if !out.success {
        bail!(trf!(
            "新版本没有通过 Gatekeeper 检查（{}），已拒绝安装",
            "The new version didn't pass Gatekeeper ({}); refusing to install",
            short_reason(&out)
        ));
    }
    Ok(())
}

/// 怎样安装：原地替换，还是只能下载 dmg 让用户自己拖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Replace { bundle: PathBuf },
    Manual { reason: String },
}

/// 可执行文件所在的 `.app`（`Helmp.app/Contents/MacOS/helmp` → `Helmp.app`）。
pub fn running_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension().is_some_and(|ext| ext == "app"))
    .then(|| app.to_path_buf())
}

/// 决定安装方式。`writable` 判断目录是否可写（测试里注入）。
pub fn plan_location(
    bundle: Option<&Path>,
    home: Option<&Path>,
    writable: &dyn Fn(&Path) -> bool,
) -> Location {
    let Some(bundle) = bundle else {
        return Location::Manual {
            reason: tr(
                "当前不是从 Helmp.app 运行（开发版或命令行），无法原地替换",
                "Helmp isn't running from Helmp.app (a dev build or the command line), so it can't replace itself",
            )
            .to_string(),
        };
    };
    let Some(parent) = bundle.parent() else {
        return Location::Manual {
            reason: trf!("无法确定 {} 所在的目录", "Can't tell which folder holds {}", bundle.display()),
        };
    };
    let in_applications = parent.starts_with("/Applications")
        || home.is_some_and(|home| parent.starts_with(home.join("Applications")));
    if !in_applications {
        return Location::Manual {
            reason: trf!(
                "Helmp 不在「应用程序」文件夹里（{}），为避免替换开发版或临时副本，请手动安装",
                "Helmp isn't in an Applications folder ({}); to avoid replacing a dev build or a temporary copy, install it manually",
                bundle.display()
            ),
        };
    }
    if !writable(parent) {
        return Location::Manual {
            reason: trf!(
                "没有写入 {} 的权限，无法替换 Helmp",
                "No permission to write to {}, so Helmp can't be replaced",
                parent.display()
            ),
        };
    }
    Location::Replace {
        bundle: bundle.to_path_buf(),
    }
}

pub fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: path 是以 NUL 结尾的有效 C 字符串，access 不保留指针。
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// 当前进程的安装方式。演练模式下可用 `HELMP_UPDATE_TARGET` 假装从某个 `.app` 运行（测试用，
/// 正式运行时忽略，不能借环境变量让 Helmp 去替换别的 App）。
pub fn current_location() -> Location {
    let bundle = match std::env::var_os("HELMP_UPDATE_TARGET") {
        Some(target) if crate::effects::dry_run() => Some(PathBuf::from(target)),
        _ => std::env::current_exe()
            .ok()
            .and_then(|exe| running_bundle(&exe)),
    };
    let home = crate::paths::home_dir().ok();
    plan_location(bundle.as_deref(), home.as_deref(), &dir_writable)
}

/// 解压目录：`<App 所在目录>/.helmp-update-<版本>-<pid>`。
pub fn stage_dir(bundle: &Path, version: &Version) -> Result<PathBuf> {
    let parent = bundle
        .parent()
        .with_context(|| trf!("无法确定 {} 所在的目录", "Can't tell which folder holds {}", bundle.display()))?;
    Ok(parent.join(format!("{STAGE_PREFIX}{version}-{}", std::process::id())))
}

/// 解压 zip 到 App 旁边的隐藏目录并校验；失败时删掉解压目录。返回解压出的 `Helmp.app`。
pub fn stage(
    zip: &Path,
    bundle: &Path,
    version: &Version,
    runner: &dyn Runner,
    expected_team: &str,
) -> Result<PathBuf> {
    let dir = stage_dir(bundle, version)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir)
        .with_context(|| trf!("无法创建 {}", "Couldn't create {}", dir.display()))?;
    let result = (|| -> Result<PathBuf> {
        let out = std::process::Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(zip)
            .arg(&dir)
            .output()
            .context("ditto")?;
        if !out.status.success() {
            bail!(trf!(
                "解压失败：{}",
                "Couldn't unpack the update: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let app = dir.join("Helmp.app");
        if !app.is_dir() {
            bail!(tr(
                "安装包里没有 Helmp.app",
                "The update doesn't contain Helmp.app"
            ));
        }
        verify_app(runner, &app, expected_team, version)?;
        Ok(app)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// 助手脚本：参数为 Helmp 的 pid、要替换的 App、新版本、是否重新打开、日志文件。
/// 只接受以 .app 结尾的路径，只删除 `.helmp-update-` 开头的解压目录与自己改名出来的旧版本。
const HELPER: &str = r#"#!/bin/sh
# Helmp 更新助手（由 Helmp 生成）：等 Helmp 退出后用新版本替换旧版本，再重新打开。
pid="$1"; target="$2"; staged="$3"; relaunch="$4"; log="$5"
exec >>"$log" 2>&1
case "$target" in /*.app) ;; *) echo "目标路径无效：$target"; exit 2 ;; esac
case "$staged" in /*/.helmp-update-*/Helmp.app) ;; *) echo "新版本路径无效：$staged"; exit 2 ;; esac
stagedir="$(dirname "$staged")"
i=0
while kill -0 "$pid" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -gt 600 ]; then
    echo "Helmp 60 秒内没有退出，放弃这次更新"
    rm -rf "$stagedir"
    exit 1
  fi
  sleep 0.1
done
backup="$target.old-$$"
if ! mv "$target" "$backup"; then
  echo "无法移走旧版本：$target"
  rm -rf "$stagedir"
  [ "$relaunch" = 1 ] && open "$target"
  exit 1
fi
if mv "$staged" "$target"; then
  rm -rf "$backup"
  echo "已更新：$target"
else
  echo "放入新版本失败，恢复旧版本"
  mv "$backup" "$target"
fi
rm -rf "$stagedir"
[ "$relaunch" = 1 ] && open "$target"
rm -f "$0"
"#;

/// 启动助手（脱离 Helmp 的进程组，Helmp 退出后继续运行）。返回日志路径。
pub fn spawn_helper(pid: u32, bundle: &Path, staged: &Path, relaunch: bool) -> Result<PathBuf> {
    use std::os::unix::process::CommandExt as _;
    let work = std::env::temp_dir().join(format!("helmp-update-{}", std::process::id()));
    std::fs::create_dir_all(&work)
        .with_context(|| trf!("无法创建 {}", "Couldn't create {}", work.display()))?;
    let script = work.join("replace.sh");
    let log = work.join("update.log");
    std::fs::write(&script, HELPER)
        .with_context(|| trf!("无法写入 {}", "Couldn't write {}", script.display()))?;
    std::process::Command::new("/bin/sh")
        .arg(&script)
        .arg(pid.to_string())
        .arg(bundle)
        .arg(staged)
        .arg(if relaunch { "1" } else { "0" })
        .arg(&log)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| tr("无法启动更新助手", "Couldn't start the update helper"))?;
    Ok(log)
}

/// 原地替换的步骤说明（演练模式打印，也写进 `helmp update install` 的计划）。
pub fn replace_steps(bundle: &Path, version: &Version) -> Vec<String> {
    let stage = stage_dir(bundle, version)
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    vec![
        trf!(
            "用 ditto 解压到 {stage}/",
            "Unpack with ditto into {stage}/"
        ),
        trf!(
            "校验：bundle id {BUNDLE_ID}、版本 {version}、codesign --verify --deep --strict、Team ID 与当前 Helmp 一致、spctl --assess --type execute",
            "Verify: bundle id {BUNDLE_ID}, version {version}, codesign --verify --deep --strict, Team ID matches this Helmp, spctl --assess --type execute"
        ),
        trf!(
            "Helmp 退出后替换 {}（旧版本先改名为 *.old 再删除），然后重新打开",
            "After Helmp quits, replace {} (the old version is renamed to *.old, then deleted) and reopen it",
            bundle.display()
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 假的 codesign / spctl：按程序和参数回复，记录调用。
    struct FakeRunner {
        team: Option<&'static str>,
        verify_ok: bool,
        spctl_ok: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeRunner {
        fn good(team: &'static str) -> Self {
            Self { team: Some(team), verify_ok: true, spctl_ok: true, calls: RefCell::new(Vec::new()) }
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
            self.calls.borrow_mut().push(format!("{program} {}", args.join(" ")));
            let ok = |success: bool, stderr: &str| CmdOutput {
                success,
                stdout: String::new(),
                stderr: stderr.to_string(),
            };
            Ok(match (program, args.first().copied()) {
                ("/usr/bin/codesign", Some("-dv")) => ok(
                    true,
                    &format!(
                        "Executable=/x\nIdentifier=dev.helmp.launcher\nTeamIdentifier={}\n",
                        self.team.unwrap_or("not set")
                    ),
                ),
                ("/usr/bin/codesign", Some("--verify")) => {
                    ok(self.verify_ok, if self.verify_ok { "valid on disk" } else { "a sealed resource is missing or invalid" })
                }
                ("/usr/sbin/spctl", _) => ok(self.spctl_ok, if self.spctl_ok { "accepted" } else { "rejected" }),
                _ => panic!("意外的命令：{program} {args:?}"),
            })
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helmp-install-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 造一个只有 Info.plist 的假 Helmp.app。
    fn fake_app(dir: &Path, bundle_id: &str, version: &str) -> PathBuf {
        let app = dir.join("Helmp.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
        );
        std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        std::fs::write(app.join("Contents/MacOS/helmp"), "#!/bin/sh\n").unwrap();
        app
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn team_id_is_read_from_codesign_output() {
        let app = Path::new("/Applications/Helmp.app");
        assert_eq!(team_id(&FakeRunner::good("R85345VS2J"), app).unwrap().as_deref(), Some("R85345VS2J"));
        let unsigned = FakeRunner { team: None, ..FakeRunner::good("x") };
        assert_eq!(team_id(&unsigned, app).unwrap(), None);
    }

    #[test]
    fn genuine_update_passes_all_checks() {
        let dir = temp("genuine");
        let app = fake_app(&dir, BUNDLE_ID, "0.4.0");
        let runner = FakeRunner::good("R85345VS2J");
        verify_app(&runner, &app, "R85345VS2J", &v("0.4.0")).unwrap();
        let calls = runner.calls.borrow().join("\n");
        for needle in ["--verify --deep --strict", "-dv", "spctl --assess --type execute"] {
            assert!(calls.contains(needle), "缺少 {needle}：{calls}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn foreign_team_id_is_refused() {
        // 更新源被篡改、换成别人签名（哪怕签名本身有效、也公证过）的 App：必须拒绝。
        let dir = temp("foreign");
        let app = fake_app(&dir, BUNDLE_ID, "0.4.0");
        let err = format!(
            "{:#}",
            verify_app(&FakeRunner::good("EVILTEAM01"), &app, "R85345VS2J", &v("0.4.0")).unwrap_err()
        );
        assert!(err.contains("EVILTEAM01") && err.contains("R85345VS2J"), "{err}");
        let unsigned = FakeRunner { team: None, ..FakeRunner::good("x") };
        assert!(verify_app(&unsigned, &app, "R85345VS2J", &v("0.4.0")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn broken_signature_or_gatekeeper_rejection_is_refused() {
        let dir = temp("broken");
        let app = fake_app(&dir, BUNDLE_ID, "0.4.0");
        let bad_sig = FakeRunner { verify_ok: false, ..FakeRunner::good("R85345VS2J") };
        assert!(format!("{:#}", verify_app(&bad_sig, &app, "R85345VS2J", &v("0.4.0")).unwrap_err()).contains("sealed"));
        let bad_gk = FakeRunner { spctl_ok: false, ..FakeRunner::good("R85345VS2J") };
        assert!(format!("{:#}", verify_app(&bad_gk, &app, "R85345VS2J", &v("0.4.0")).unwrap_err()).contains("rejected"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wrong_bundle_or_version_is_refused_before_running_codesign() {
        // 回滚攻击：把旧版本的（真实签名的）包挂在新版本号下面，也要拒绝。
        let dir = temp("version");
        let app = fake_app(&dir, BUNDLE_ID, "0.2.0");
        let runner = FakeRunner::good("R85345VS2J");
        assert!(verify_app(&runner, &app, "R85345VS2J", &v("0.4.0")).is_err());
        let other = temp("bundle");
        let app = fake_app(&other, "com.evil.app", "0.4.0");
        assert!(verify_app(&runner, &app, "R85345VS2J", &v("0.4.0")).is_err());
        assert!(runner.calls.borrow().is_empty(), "Info.plist 不对时不应再跑 codesign");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(other);
    }

    #[test]
    fn staging_removes_everything_when_verification_fails() {
        // 校验不过时，「应用程序」文件夹里不能留下任何解压出来的东西。
        let root = temp("stage");
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        fake_app(&src, BUNDLE_ID, "0.4.0");
        let zip = root.join("Helmp-0.4.0.zip");
        let status = std::process::Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(src.join("Helmp.app"))
            .arg(&zip)
            .status()
            .unwrap();
        assert!(status.success());
        let apps = root.join("Applications");
        let bundle = fake_app(&apps, BUNDLE_ID, "0.3.0");
        let before: Vec<_> = std::fs::read_dir(&apps).unwrap().map(|e| e.unwrap().file_name()).collect();

        let err = stage(&zip, &bundle, &v("0.4.0"), &FakeRunner::good("EVILTEAM01"), "R85345VS2J").unwrap_err();
        assert!(format!("{err:#}").contains("EVILTEAM01"));
        let after: Vec<_> = std::fs::read_dir(&apps).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(before, after, "失败后不能留下解压目录");

        // 同一个包、正确的签名：解压到 App 旁边的隐藏目录，旧版本原封不动。
        let staged = stage(&zip, &bundle, &v("0.4.0"), &FakeRunner::good("R85345VS2J"), "R85345VS2J").unwrap();
        assert!(staged.ends_with("Helmp.app"));
        assert!(staged.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with(".helmp-update-0.4.0-"));
        assert_eq!(bundle_info(&bundle).unwrap().1, "0.3.0");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_bundles_in_applications_folders_are_replaced_in_place() {
        let home = Path::new("/Users/me");
        let yes = |_: &Path| true;
        let no = |_: &Path| false;
        let replace = |path: &str| Location::Replace { bundle: PathBuf::from(path) };
        assert_eq!(
            plan_location(Some(Path::new("/Applications/Helmp.app")), Some(home), &yes),
            replace("/Applications/Helmp.app")
        );
        assert_eq!(
            plan_location(Some(Path::new("/Applications/Tools/Helmp.app")), Some(home), &yes),
            replace("/Applications/Tools/Helmp.app")
        );
        assert_eq!(
            plan_location(Some(Path::new("/Users/me/Applications/Helmp.app")), Some(home), &yes),
            replace("/Users/me/Applications/Helmp.app")
        );
        // 开发版（target/release）、App Translocation 的随机路径、下载目录：都不原地替换。
        for path in [
            "/Users/me/code/helmp/target/release/Helmp.app",
            "/private/var/folders/x/AppTranslocation/ABC/d/Helmp.app",
            "/Users/me/Downloads/Helmp.app",
            "/ApplicationsEvil/Helmp.app",
        ] {
            assert!(
                matches!(plan_location(Some(Path::new(path)), Some(home), &yes), Location::Manual { .. }),
                "{path}"
            );
        }
        // 没有写权限（例如标准用户、/Applications 由管理员所有）。
        assert!(matches!(
            plan_location(Some(Path::new("/Applications/Helmp.app")), Some(home), &no),
            Location::Manual { .. }
        ));
        // 不是从 .app 运行（命令行、cargo run）。
        assert!(matches!(plan_location(None, Some(home), &yes), Location::Manual { .. }));
    }

    #[test]
    fn running_bundle_is_found_from_the_executable() {
        assert_eq!(
            running_bundle(Path::new("/Applications/Helmp.app/Contents/MacOS/helmp")),
            Some(PathBuf::from("/Applications/Helmp.app"))
        );
        assert_eq!(running_bundle(Path::new("/Users/me/helmp/target/release/helmp")), None);
    }

    #[test]
    fn helper_script_is_valid_shell_and_guards_paths() {
        let dir = temp("helper");
        let script = dir.join("replace.sh");
        std::fs::write(&script, HELPER).unwrap();
        let status = std::process::Command::new("/bin/sh").arg("-n").arg(&script).status().unwrap();
        assert!(status.success(), "助手脚本语法错误");
        // 路径不对时什么都不做就退出（不会去 mv / rm 任意路径）。
        let victim = dir.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        let log = dir.join("log");
        let status = std::process::Command::new("/bin/sh")
            .arg(&script)
            .args(["999999", victim.to_str().unwrap(), "/tmp/x/Helmp.app", "0", log.to_str().unwrap()])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
        assert!(victim.is_dir());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn helper_replaces_the_bundle_after_the_process_exits() {
        // 在临时目录里完整跑一遍助手：旧版本被换成新版本，解压目录和 *.old 都被清理。
        let root = temp("swap");
        let apps = root.join("Applications");
        let bundle = fake_app(&apps, BUNDLE_ID, "0.3.0");
        let stage = apps.join(".helmp-update-0.4.0-1");
        let staged = fake_app(&stage, BUNDLE_ID, "0.4.0");
        let script = root.join("replace.sh");
        std::fs::write(&script, HELPER).unwrap();
        let log = root.join("log");
        let status = std::process::Command::new("/bin/sh")
            .arg(&script)
            .arg("999999") // 不存在的 pid：视为已退出
            .arg(&bundle)
            .arg(&staged)
            .arg("0")
            .arg(&log)
            .status()
            .unwrap();
        assert!(status.success(), "{}", std::fs::read_to_string(&log).unwrap_or_default());
        assert_eq!(bundle_info(&bundle).unwrap().1, "0.4.0");
        let left: Vec<String> = std::fs::read_dir(&apps)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["Helmp.app".to_string()]);
        let _ = std::fs::remove_dir_all(root);
    }
}
