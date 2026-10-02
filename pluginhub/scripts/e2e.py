"""端到端测试：通过 pluginhub.exe --run api 调用接口，覆盖添加、锁定、本地修改、另存并还原、后台检查修复配置、真实检查、卸载，
独立 skill 的收编、两边链接、修复和删除，以及真实检查顺带看的 Codex 技能清单。
用本地 git 仓库当远端，不碰 dclh。会真的往 Claude Code 和 Codex 里装一个叫 yp-e2e 的测试插件和一个测试 skill，
测完会卸掉并清理干净，最后核对两边的配置和测试前一样。

先构建 exe（cargo build --release），再运行：python scripts/e2e.py
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

EXE = Path(__file__).resolve().parents[1] / "src-tauri" / "target" / "release" / "pluginhub.exe"
HOME = Path.home()
HUB_DIR = HOME / ".pluginhub"
PLUGINS_DIR = HOME / ".yuwanplugins"
SAVED_DIR = HUB_DIR / "saved"
BACKUP_DIR = HUB_DIR / "backups"
CLAUDE_SKILLS = HOME / ".claude" / "skills"
CODEX_HOME = HOME / ".codex"
work = Path(tempfile.gettempdir()) / "pluginhub-e2e"
CODEX_CONFIG_BEFORE = (Path.home() / ".codex" / "config.toml").read_text(encoding="utf-8")
SETTINGS = HOME / ".claude" / "settings.json"
SETTINGS_BEFORE = json.loads(SETTINGS.read_text(encoding="utf-8"))
src, bare = work / "src", work / "yp-e2e.git"
g = shutil.which("git")
ID = "yp-e2e"
failed = False


ENV = {**os.environ, "PLUGINHUB_NO_OPEN": "1"}  # 另存并还原时不弹出资源管理器


def api(name, body=None):
    r = subprocess.run([str(EXE), "--run", "api", name, json.dumps(body or {})], capture_output=True, text=True,
                       encoding="utf-8", env=ENV)
    out = r.stdout.strip().splitlines()
    data = json.loads(out[-1]) if out else {}
    if r.returncode != 0:
        print(f"   api {name} 失败（{r.returncode}）：{data.get('error')}")
    return data


def notes_of(name, body=None):
    return api(name, body).get("notes") or []


def git(*args, cwd=src):
    return subprocess.run([g, "-c", "user.name=t", "-c", "user.email=t@t", *args], cwd=cwd, check=True,
                          capture_output=True, text=True).stdout.strip()


def commit_remote(msg, text):
    (src / "skills" / "hello" / "SKILL.md").write_text(f"---\nname: hello\ndescription: e2e skill\n---\n{text}\n")
    git("commit", "-am", msg)
    git("push", "origin", "main")


def row():
    return next(r for r in api("state")["plugins"] if r["key"] == ID)


def check(label, ok):
    global failed
    print(("PASS " if ok else "FAIL ") + label)
    if not ok:
        failed = True


def rmtree(p):
    if p.exists():
        shutil.rmtree(p, onerror=lambda f, x, e: (os.chmod(x, 0o666), f(x)))


def is_link_to(link, target):
    try:
        t = os.readlink(link)
    except OSError:
        return False
    t = t.removeprefix("\\\\?\\")
    return os.path.normcase(os.path.abspath(t)) == os.path.normcase(os.path.abspath(target))


rmtree(work)
(src / ".claude-plugin").mkdir(parents=True)
(src / ".codex-plugin").mkdir()
(src / "skills" / "hello").mkdir(parents=True)
(src / ".claude-plugin" / "plugin.json").write_text(json.dumps({"name": ID, "version": "0.0.1", "description": "e2e"}))
(src / ".codex-plugin" / "plugin.json").write_text(json.dumps({"name": ID, "version": "0.0.1", "description": "e2e", "skills": "./skills/"}))
(src / "skills" / "hello" / "SKILL.md").write_text("---\nname: hello\ndescription: e2e skill\n---\nv1\n")
git("init", "-b", "main")
git("add", ".")
git("commit", "-m", "v1")
subprocess.run([g, "clone", "--bare", str(src), str(bare)], check=True, capture_output=True)
git("remote", "add", "origin", str(bare))
url = "file:///" + str(bare).replace("\\", "/")
folder = PLUGINS_DIR / ID

# 0. 查询仓库
probe = api("probe", {"repo": url})
check("probe 列出 main 分支和版本号", any(b["name"] == "main" and b["version"] == "0.0.1" and b["claude"] and b["codex"] for b in probe.get("branches", [])))

# 1. 添加：两边都链接，装完的真实检查要通过（add 里 strict）
res = api("add", {"repo": url, "branch": "main", "apps": ["claude", "codex"]})
print("   add:", "\n        ".join(res.get("notes") or [res.get("error")]))
r = row()
check("添加后两边都是最新", {a["app"]: a.get("state") for a in r["apps"]} == {"claude": "ok", "codex": "ok"})
check("添加后真实检查都通过", all((a.get("verify") or {}).get("ok") for a in r["apps"]))
budget = api("state").get("codex_skills") or {}
print("   Codex 的技能清单：", budget.get("detail"))
check("真实检查顺带看了 Codex 的技能清单", bool(budget.get("at")) and "个技能" in budget.get("detail", ""))
check("Claude 链接指向插件文件夹", (CLAUDE_SKILLS / ID).is_dir() and Path(os.readlink(CLAUDE_SKILLS / ID)).name == ID)

# 2. 锁定：远端有新提交，检查和更新都不碰它
api("lock", {"plugin": ID, "locked": True})
commit_remote("v2", "v2")
notes = notes_of("check")
check("锁定后检查更新不报它有新版本", not any(ID in n and "有新版本" in n for n in notes))
api("update", {"plugin": ID})
check("锁定后更新不拉取", "v1" in (folder / "skills/hello/SKILL.md").read_text())
r = row()
check("锁定后不显示需要更新", r["managed"]["locked"] and not r["managed"]["needs_update"])
res = api("sync", {"key": ID})
check("锁定后同步被拒绝", "已锁定" in (res.get("error") or ""))

# 3. 解锁：恢复检查和拉取
api("lock", {"plugin": ID, "locked": False})
api("check")
check("解锁后显示有新版本", row()["managed"]["needs_update"])
api("update", {"plugin": ID})
check("解锁后拉取到 v2", "v2" in (folder / "skills/hello/SKILL.md").read_text())

# 4. 在插件文件夹里改文件：同步不拉取，并提示锁定或另存
(folder / "skills/hello/SKILL.md").write_text("---\nname: hello\ndescription: e2e skill\n---\nlocal edit\n")
check("能看出没提交的改动", row()["managed"]["modified"] == ["1 个文件有改动"])
commit_remote("v3", "v3")
notes = notes_of("update", {"plugin": ID})
check("有改动时同步不拉取并提示锁定或另存", any("锁定" in n and "另存" in n for n in notes)
      and "local edit" in (folder / "skills/hello/SKILL.md").read_text())

# 5. 本地提交也算修改（不然同步会把它冲掉）
subprocess.run([g, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-am", "local"], cwd=folder, check=True, capture_output=True)
mod = row()["managed"]["modified"]
check(f"能看出本地提交：{mod}", mod == ["1 个本地提交"])
api("update", {"plugin": ID})
check("有本地提交时同步不冲掉", "local edit" in (folder / "skills/hello/SKILL.md").read_text())

# 6. 另存并还原：修改（含本地提交）整份存出去，插件文件夹回到远端版本
before = set(SAVED_DIR.glob(f"{ID}-*")) if SAVED_DIR.exists() else set()
res = api("save", {"plugin": ID})
saved = sorted(set(SAVED_DIR.glob(f"{ID}-*")) - before)
check("另存的那份里有修改", bool(saved) and "local edit" in (saved[-1] / "skills/hello/SKILL.md").read_text())
check("另存的那份里有本地提交", bool(saved) and "local" in subprocess.run([g, "log", "-1", "--format=%s"], cwd=saved[-1], capture_output=True, text=True).stdout)
check("插件文件夹还原成远端 v3", "v3" in (folder / "skills/hello/SKILL.md").read_text() and row()["managed"]["modified"] == [])

# 7. 后台检查：模拟别的程序改掉配置——删掉 config.toml 里它的条目、删掉 Claude 的链接
cfg_toml = CODEX_HOME / "config.toml"
text = cfg_toml.read_text(encoding="utf-8")
cut = re.sub(r'\[plugins\."yp-e2e@personal"\]\r?\nenabled = true\r?\n\r?\n?', "", text)
check("找到并删掉了 config.toml 里的条目", cut != text)
cfg_toml.write_text(cut, encoding="utf-8")
os.rmdir(CLAUDE_SKILLS / ID)
r = row()
probs = {a["app"]: a.get("problems") for a in r["apps"]}
check(f"状态里写出了具体问题：{probs}", any("config.toml" in p for p in probs.get("codex") or [])
      and any("链接不见了" in p for p in probs.get("claude") or []))
notes = notes_of("guard", {"enabled": True})
print("   guard:", notes)
r = row()
check("后台检查修好了两边", {a["app"]: a.get("state") for a in r["apps"]} == {"claude": "ok", "codex": "ok"})
check("修复后做了真实检查且通过", all((a.get("verify") or {}).get("ok") for a in r["apps"]))
check("记下了修复记录", all(a.get("repair") for a in r["apps"]))
code = subprocess.run([str(EXE), "--run", "guard"], capture_output=True, text=True, encoding="utf-8")
check(f"exe --run guard 跑通且没问题时只报真实检查都通过（退出码 {code.returncode}）", code.returncode == 0 and ("都通过" in code.stdout or "都没问题" in code.stdout) and "已修复" not in code.stdout)

# 8. 后台任务的命令指向 exe 自己
st = api("state")
check("后台任务交给 exe 自己", st["hub"]["launcher"].lower().startswith(f'"{str(EXE).lower()}"') or st["hub"]["launcher"].lower().startswith(str(EXE).lower()))
print("   launcher:", st["hub"]["launcher"])
notes = notes_of("budget")
check(f"单独看 Codex 的技能清单：{notes}", len(notes) == 1 and "[技能清单] Codex" in notes[0])

# 9. 只从 Codex 卸载，再从 Claude 卸载 → 不再管理，文件夹挪进备份
notes = notes_of("uninstall", {"key": ID, "id": f"{ID}@personal", "app": "codex"})
r = row()
check("从 Codex 卸掉后 Claude 还在", {a["app"]: a["installed"] for a in r["apps"]}.get("claude") and not any(a["app"] == "codex" and a["installed"] for a in r["apps"]))
notes = notes_of("uninstall", {"key": ID})
check("全部卸掉后不再管理", not any(r["key"] == ID for r in api("state")["plugins"]) and not folder.exists())
check("插件文件夹挪进了备份", any(BACKUP_DIR.glob(f"{ID}-*")))

# 10. 独立的 skill：只装在 Codex 里的，装到 Claude Code → 挪进 ~/.yuwanplugins/skills 统一存放，两边都换成链接
SK = f"{ID}-skill"
skill_src = CODEX_HOME / "skills" / SK
ours_skill = PLUGINS_DIR / "skills" / SK
rmtree(skill_src)
skill_src.mkdir(parents=True)
(skill_src / "SKILL.md").write_text("---\nname: yp-e2e-skill\ndescription: 'e2e test skill, it''s temporary'\n---\nhello\n", encoding="utf-8")


def srow():
    return next((r for r in api("state")["plugins"] if r["key"] == f"skill:{SK}"), None)


r = srow()
check("独立的 skill 出现在列表里，只装在 Codex", r is not None and r["kind"] == "skill" and [a["app"] for a in r["apps"] if a["installed"]] == ["codex"])
check("SKILL.md 开头单引号里的 '' 读对了", r is not None and r["description"] == "e2e test skill, it's temporary")
check("能装到 Claude Code，方式是先挪进统一存放的地方", r is not None and (r["install"].get("claude") or {}).get("how") == "move")
res = api("install", {"key": f"skill:{SK}", "apps": ["claude"]})
print("   skill install:", res.get("notes") or res.get("error"))
r = srow()
check("统一存放在 ~/.yuwanplugins/skills", (ours_skill / "SKILL.md").is_file() and r["skill"]["ours"])
check("两边都是指向它的链接", is_link_to(CLAUDE_SKILLS / SK, ours_skill) and is_link_to(skill_src, ours_skill))
check("两边都显示已链接", {a["app"]: a.get("state") for a in r["apps"]} == {"claude": "ok", "codex": "ok"})
check("装完的真实检查都通过", all((a.get("verify") or {}).get("ok") for a in r["apps"]))
os.rmdir(CLAUDE_SKILLS / SK)
r = srow()
check("链接被删以后显示出问题", any(a["app"] == "claude" and a.get("state") == "missing" for a in r["apps"]))
notes = notes_of("guard", {"enabled": True})
check("后台检查把 skill 的链接修好了", is_link_to(CLAUDE_SKILLS / SK, ours_skill) and any(SK in n and "已修复" in n for n in notes))
res = api("sync", {"key": f"skill:{SK}"})
check("同步（补链接并检查）不报错", "error" not in res)

# 11. 删技能：先只从 Codex 删，再从所有 app 删
api("uninstall", {"key": f"skill:{SK}", "id": f"skill:{SK}", "app": "codex"})
check("只从 Codex 删：拆了链接，统一存放的那份还在", not skill_src.exists() and ours_skill.is_dir() and is_link_to(CLAUDE_SKILLS / SK, ours_skill))
api("uninstall", {"key": f"skill:{SK}"})
check("从所有 app 删掉后不再管理", srow() is None and not ours_skill.exists() and not (CLAUDE_SKILLS / SK).exists())
check("统一存放的那份挪进了备份", any(BACKUP_DIR.glob(f"skill-{SK}-*")))
check("Codex 的 config.toml 和测试前一样", (CODEX_HOME / "config.toml").read_text(encoding="utf-8") == CODEX_CONFIG_BEFORE)
check("Claude Code 的 settings.json 和测试前一样", json.loads(SETTINGS.read_text(encoding="utf-8")) == SETTINGS_BEFORE)

# 收尾：去掉测试插件和测试 skill 在 state.json 里的记录，备份和另存的文件夹，测试用的仓库
st_path = HUB_DIR / "state.json"
st = json.loads(st_path.read_text(encoding="utf-8"))
for k in ("verify", "repairs"):
    for key in [x for x in st.get(k, {}) if ID in x]:
        del st[k][key]
st.get("plugins", {}).pop(ID, None)
st_path.write_text(json.dumps(st, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
for d in [*BACKUP_DIR.glob(f"{ID}-*"), *BACKUP_DIR.glob(f"skill-{SK}-*"), *(SAVED_DIR.glob(f"{ID}-*") if SAVED_DIR.exists() else [])]:
    rmtree(d)
for d in (PLUGINS_DIR / "skills",):
    if d.exists() and not any(d.iterdir()):
        d.rmdir()
if SAVED_DIR.exists() and not any(SAVED_DIR.iterdir()):
    SAVED_DIR.rmdir()
rmtree(work)

print("RESULT:", "FAILED" if failed else "ALL PASSED")
sys.exit(1 if failed else 0)
