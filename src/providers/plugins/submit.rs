//! `helmp plugins submit <目录> [--yes]`：把插件以 Pull Request 的形式提交到插件中心注册表。
//!
//! 全程用用户自己的 `gh` CLI 和 git（Helmp 不保存任何 GitHub 凭据）：
//! 1. 校验清单与文件（与注册表 CI 同样的规则），算出权限与文件清单。
//! 2. 不带 `--yes` 时只打印将要做什么，然后以失败退出——提交必须由用户明确确认（agent 也要先问用户）。
//! 3. `gh auth status` 确认已登录；对注册表有推送权限就直接推分支，否则 `gh repo fork` 后推到自己的 fork。
//! 4. 在临时目录 `gh repo clone`，建分支 `plugin/<id>-<版本>`，把文件放进 `plugins/<id>/`，提交、推送，
//!    `gh pr create` 开 PR（正文按注册表的 PR 模板填好权限和文件清单），打印 PR 地址。
//!
//! 注册表仓库默认 PeterChen1997/helmp-plugins，可用 `HELMP_REGISTRY_REPO` 改（测试指向假仓库，
//! 并在 PATH 里放一个假的 `gh`，不碰真实 GitHub）。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, anyhow, bail};

use super::center::{self, FileEntry};
use super::registry::{self, Source};
use crate::i18n::{Lang, tr, trf, with_lang};

pub const DEFAULT_REPO: &str = "PeterChen1997/helmp-plugins";

pub fn repo() -> Result<String> {
    let repo = std::env::var("HELMP_REGISTRY_REPO")
        .ok()
        .filter(|repo| !repo.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let valid = repo.split_once('/').is_some_and(|(owner, name)| {
        let ok = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        ok(owner) && ok(name) && !name.contains('/')
    });
    if !valid {
        bail!(trf!(
            "注册表仓库 {repo:?} 不是 owner/name 形式",
            "Registry repo {repo:?} isn't in owner/name form"
        ));
    }
    Ok(repo)
}

/// 提交计划：校验通过后才有。
#[derive(Debug, Clone)]
pub struct Plan {
    pub dir: PathBuf,
    pub id: String,
    pub version: String,
    pub author: String,
    pub license: Option<String>,
    pub permissions_en: Vec<String>,
    pub permissions_zh: Vec<String>,
    pub files: Vec<FileEntry>,
    pub repo: String,
    pub branch: String,
}

pub fn plan(dir: &Path) -> Result<Plan> {
    let entry = registry::load_dir(dir, Source::User);
    let plugin = entry.loaded.map_err(|errors| {
        anyhow!(trf!(
            "插件校验失败，没有提交：\n  - {}",
            "Plugin validation failed, nothing submitted:\n  - {}",
            errors.join("\n  - ")
        ))
    })?;
    let mut problems = Vec::new();
    if plugin
        .manifest
        .license
        .as_deref()
        .is_none_or(|l| l.trim().is_empty())
    {
        problems.push(
            tr(
                "plugin.json 要写 license（如 \"MIT\"），插件中心要求声明协议",
                "plugin.json needs a license (e.g. \"MIT\"); the plugin center requires one",
            )
            .to_string(),
        );
    }
    let files = match center::collect_files(dir) {
        Ok(files) => files,
        Err(errors) => {
            problems.extend(errors);
            Vec::new()
        }
    };
    if !problems.is_empty() {
        bail!(trf!(
            "插件还不能提交：\n  - {}",
            "The plugin can't be submitted yet:\n  - {}",
            problems.join("\n  - ")
        ));
    }
    // 注册表里的文件夹名总是 id，本地文件夹叫什么都行。
    let permissions = &plugin.permissions;
    Ok(Plan {
        dir: dir.to_path_buf(),
        id: plugin.id().to_string(),
        version: plugin.manifest.version.clone(),
        author: plugin.manifest.author.clone(),
        license: plugin.manifest.license.clone(),
        permissions_en: with_lang(Lang::En, || permissions.describe()),
        permissions_zh: with_lang(Lang::Zh, || permissions.describe()),
        files,
        repo: repo()?,
        branch: format!("plugin/{}-{}", plugin.id(), plugin.manifest.version),
    })
}

impl Plan {
    /// 给用户（或替用户操作的 agent）看的说明：会发生什么。
    pub fn describe(&self) -> String {
        let mut out = String::new();
        out.push_str(&trf!(
            "将要把插件 {} {} 提交到插件中心（{}）：\n",
            "About to submit plugin {} {} to the plugin center ({}):\n",
            self.id,
            self.version,
            self.repo
        ));
        out.push_str(&trf!(
            "  1. 用你的 gh 账号：有推送权限就直接推分支，否则先 fork {}\n",
            "  1. Use your gh account: push a branch directly if you have push access, otherwise fork {} first\n",
            self.repo
        ));
        out.push_str(&trf!(
            "  2. 新建分支 {}，把下面的文件放进 plugins/{}/ 并提交\n",
            "  2. Create branch {} and commit the files below into plugins/{}/\n",
            self.branch,
            self.id
        ));
        out.push_str(tr(
            "  3. 开一个 Pull Request，等维护者人工审核后上架\n",
            "  3. Open a pull request; a maintainer reviews it before it's listed\n",
        ));
        let lines = if crate::i18n::is_en() {
            &self.permissions_en
        } else {
            &self.permissions_zh
        };
        out.push_str(tr("权限：\n", "Permissions:\n"));
        if lines.is_empty() {
            out.push_str(tr("  - 无\n", "  - none\n"));
        }
        for line in lines {
            out.push_str(&format!("  - {line}\n"));
        }
        out.push_str(tr("文件：\n", "Files:\n"));
        for file in &self.files {
            let exec = if file.executable {
                tr("（可执行）", " (executable)")
            } else {
                ""
            };
            out.push_str(&format!("  - {} · {} B{exec}\n", file.path, file.size));
        }
        out
    }

    fn pr_body(&self, update: bool) -> String {
        let kind = if update {
            "Update / 更新"
        } else {
            "New plugin / 新插件"
        };
        let mut body = format!(
            "## Plugin / 插件\n\n- ID: `{}`\n- Version / 版本: `{}`\n- Type / 类型: {kind}\n- Author / 作者: {}\n- License / 协议: {}\n\n## Permissions / 权限\n\n",
            self.id,
            self.version,
            self.author,
            self.license.as_deref().unwrap_or("-")
        );
        if self.permissions_en.is_empty() {
            body.push_str("- None / 无\n");
        }
        for (en, zh) in self.permissions_en.iter().zip(&self.permissions_zh) {
            body.push_str(&format!("- {en} / {zh}\n"));
        }
        body.push_str(
            "\n## Files / 文件\n\n| Path | Size | SHA-256 | Executable |\n|---|---|---|---|\n",
        );
        for file in &self.files {
            body.push_str(&format!(
                "| `{}` | {} | `{}` | {} |\n",
                file.path,
                file.size,
                file.sha256,
                if file.executable { "yes" } else { "no" }
            ));
        }
        body.push_str(
            "\n## Reviewer checklist / 审核清单\n\n\
             - [ ] Permissions are reasonable for what the plugin does / 权限合理，与插件用途相符\n\
             - [ ] Every host it reaches is expected / 访问的主机都在预期之内\n\
             - [ ] Scripts aren't obfuscated, don't download and run remote code, and don't touch sensitive paths / 脚本没有混淆，不下载执行远程代码，不读写敏感路径\n\
             - [ ] Works as the README describes / 按 README 实测可用\n\
             - [ ] License is declared / 已声明协议\n\
             - [ ] Version was bumped (updates) / 版本号已提升（更新时）\n\n\
             Submitted with `helmp plugins submit`.\n",
        );
        body
    }
}

/// 运行外部命令，失败时把 stderr 带进错误。返回去掉首尾空白的 stdout。
fn run(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let shown = format!("{program} {}", args.join(" "));
    let out = command.output().with_context(|| {
        if program == "gh" {
            tr(
                "找不到 gh 命令：先安装 GitHub CLI（brew install gh）并运行 gh auth login",
                "gh not found: install the GitHub CLI (brew install gh) and run gh auth login",
            )
            .to_string()
        } else {
            trf!("无法运行 {shown}", "Couldn't run {shown}")
        }
    })?;
    if !out.status.success() {
        bail!(trf!(
            "{shown} 失败：{}",
            "{shown} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 真正提交：返回 PR 地址。调用前用户必须已经确认（`--yes`）。
pub fn submit(plan: &Plan) -> Result<String> {
    run("gh", &["auth", "status"], None).map_err(|err| {
        anyhow!(trf!(
            "gh 还没登录 GitHub：先运行 gh auth login（{err:#}）",
            "gh isn't signed in to GitHub: run gh auth login first ({err:#})"
        ))
    })?;
    let login = run("gh", &["api", "user", "--jq", ".login"], None)?;
    let repo = &plan.repo;
    let can_push = run(
        "gh",
        &["api", &format!("repos/{repo}"), "--jq", ".permissions.push"],
        None,
    )
    .is_ok_and(|out| out == "true");

    let work = std::env::temp_dir().join(format!(
        "helmp-submit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let result = submit_in(plan, &work, &login, can_push);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn submit_in(plan: &Plan, work: &Path, login: &str, can_push: bool) -> Result<String> {
    let repo = &plan.repo;
    let clone = work.join("registry");
    std::fs::create_dir_all(work)?;
    let clone_str = clone.to_string_lossy().into_owned();
    run("gh", &["repo", "clone", repo, &clone_str], None)?;
    let git = |args: &[&str]| run("git", args, Some(&clone));
    git(&["checkout", "-b", &plan.branch])?;
    let target = clone.join("plugins").join(&plan.id);
    let update = target.join("plugin.json").is_file();
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    for file in &plan.files {
        let to = target.join(&file.path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(plan.dir.join(&file.path), &to)
            .with_context(|| trf!("复制 {}", "Couldn't copy {}", file.path))?;
    }
    let rel = format!("plugins/{}", plan.id);
    git(&["add", "-A", "--", &rel])?;
    if git(&["diff", "--cached", "--quiet"]).is_ok() {
        bail!(trf!(
            "和插件中心里的 {} 完全相同，没有需要提交的改动",
            "Identical to {} in the plugin center; nothing to submit",
            plan.id
        ));
    }
    let verb = if update { "update" } else { "add" };
    let title = format!("plugin: {verb} {} {}", plan.id, plan.version);
    git(&["commit", "-q", "-m", &title])?;

    let (remote, head) = if can_push {
        ("origin".to_string(), plan.branch.clone())
    } else {
        run(
            "gh",
            &["repo", "fork", repo, "--clone=false", "--remote=false"],
            None,
        )?;
        let name = repo.split_once('/').map(|(_, name)| name).unwrap_or(repo);
        let url = run(
            "gh",
            &[
                "repo",
                "view",
                &format!("{login}/{name}"),
                "--json",
                "url",
                "--jq",
                ".url",
            ],
            None,
        )?;
        (url, format!("{login}:{}", plan.branch))
    };
    git(&["push", "-q", &remote, &format!("HEAD:refs/heads/{}", plan.branch)]).map_err(|err| {
        anyhow!(trf!(
            "推送分支 {} 失败（同一版本提交过一次的话，先提升 version 再提交）：{err:#}",
            "Couldn't push branch {} (if this version was submitted before, bump version first): {err:#}",
            plan.branch
        ))
    })?;
    let body_path = work.join("pr-body.md");
    std::fs::write(&body_path, plan.pr_body(update))?;
    let out = run(
        "gh",
        &[
            "pr",
            "create",
            "--repo",
            repo,
            "--base",
            "main",
            "--head",
            &head,
            "--title",
            &title,
            "--body-file",
            &body_path.to_string_lossy(),
        ],
        None,
    )?;
    out.lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("http"))
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow!(trf!(
                "gh pr create 没有返回 PR 地址：{out}",
                "gh pr create printed no PR URL: {out}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_body_lists_permissions_files_and_an_unchecked_checklist() {
        // 维护者审核时要一眼看到权限和每个文件的哈希；清单必须是未勾选的，由人来勾。
        let plan = Plan {
            dir: PathBuf::from("/x"),
            id: "com.example.memo".into(),
            version: "1.0.0".into(),
            author: "A".into(),
            license: Some("MIT".into()),
            permissions_en: vec!["Run scripts".into()],
            permissions_zh: vec!["运行脚本".into()],
            files: vec![FileEntry {
                path: "bin/run.sh".into(),
                sha256: "c".repeat(64),
                size: 12,
                executable: true,
            }],
            repo: DEFAULT_REPO.into(),
            branch: "plugin/com.example.memo-1.0.0".into(),
        };
        let body = plan.pr_body(false);
        assert!(body.contains("- Run scripts / 运行脚本"), "{body}");
        assert!(body.contains(&format!(
            "| `bin/run.sh` | 12 | `{}` | yes |",
            "c".repeat(64)
        )));
        assert!(body.contains("- [ ] Permissions are reasonable"));
        assert!(!body.contains("[x]"));
    }
}
