"""端到端测试：通过 pluginhub --run api（Windows 上是 pluginhub.exe）调用接口，覆盖添加、锁定、本地修改、另存并还原、后台检查修复配置、真实检查、卸载，
开机自启的后台进程跟着程序换位置，
独立 skill 的收编、两边链接、修复和删除，真实检查顺带看的 Codex 技能清单，以及用 npx skills add 命令从 git 仓库装技能。
用本地 git 仓库当远端，不碰 dclh。会真的往 Claude Code 和 Codex 里装一个叫 yp-e2e 的测试插件和一个测试 skill，
测完会卸掉并清理干净，关掉测试里开的后台检查，最后核对两边的配置和测试前一样。

先构建程序（cargo build --release），再运行：python scripts/e2e.py
（Windows 和 macOS 都能跑；程序不在默认位置时用环境变量 PLUGINHUB_EXE 指定）
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

EXE = Path(os.environ.get("PLUGINHUB_EXE") or Path(__file__).resolve().parents[1] / "src-tauri" / "target" / "release" / ("pluginhub.exe" if os.name == "nt" else "pluginhub"))
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


ENV = {**os.environ, "PLUGINHUB_NO_OPEN": "1"}  # 另存并还原时不弹出资源管理器（访达）


def api(name, body=None):
    """调用 pluginhub 的 api 接口，返回它打印的 JSON。[基础设施]

    Args:
        name: 接口名，比如 state、add、guard
        body: 接口参数，None 当作 {}
    Returns:
        dict — 接口输出的 JSON；进程退出码不为 0 时打印错误，照样返回
    """
    r = subprocess.run([str(EXE), "--run", "api", name, json.dumps(body or {})], capture_output=True, text=True,
                       encoding="utf-8", env=ENV)
    out = r.stdout.strip().splitlines()
    data = json.loads(out[-1]) if out else {}
    if r.returncode != 0:
        print(f"   api {name} 失败（{r.returncode}）：{data.get('error')}")
    return data


def notes_of(name, body=None):
    """调用接口，只取返回里的 notes。[基础设施]

    Args:
        name: 接口名
        body: 接口参数，None 当作 {}
    Returns:
        list[str] — notes，没有就是空列表
    """
    return api(name, body).get("notes") or []


def git(*args, cwd=src):
    """在 cwd 里跑一条 git 命令，带固定的测试用身份。[基础设施]

    Args:
        *args: git 的子命令和参数
        cwd: 仓库目录，默认是测试用的源仓库
    Returns:
        str — 标准输出去掉首尾空白；命令失败直接抛 CalledProcessError
    """
    return subprocess.run([g, "-c", "user.name=t", "-c", "user.email=t@t", *args], cwd=cwd, check=True,
                          capture_output=True, text=True).stdout.strip()


def commit_remote(msg, text):
    """往测试远端提交一版 hello skill 的新内容。[基础设施]

    Args:
        msg: 提交说明
        text: 写进 SKILL.md 正文的内容
    """
    (src / "skills" / "hello" / "SKILL.md").write_text(f"---\nname: hello\ndescription: e2e skill\n---\n{text}\n")
    git("commit", "-am", msg)
    git("push", "origin", "main")


def row():
    """state 里 yp-e2e 这一行。[基础设施]

    Returns:
        dict — 插件行；找不到直接抛 StopIteration
    """
    return next(r for r in api("state")["plugins"] if r["key"] == ID)


def check(label, ok):
    """打印 PASS 或 FAIL，记下有没有失败过。[基础设施]

    Args:
        label: 这一项检查的名字
        ok: 真就是通过
    """
    global failed
    print(("PASS " if ok else "FAIL ") + label)
    if not ok:
        failed = True


def rmtree(p):
    """删掉整个目录，.git 里的只读文件也删。[基础设施]

    Args:
        p: 目录路径，不存在就什么都不做
    """
    if p.exists():
        shutil.rmtree(p, onerror=lambda f, x, e: (os.chmod(x, 0o666), f(x)))


def is_link_to(link, target):
    """link 是不是指向 target 的目录链接。[基础设施]

    Args:
        link: 链接路径，不是链接返回 False
        target: 期望指向的目录
    Returns:
        bool

    变更: 2026-10-03 相对路径的链接按链接所在目录解析，绝对路径的照旧。
    """
    try:
        t = os.readlink(link)
    except OSError:
        return False
    t = t.removeprefix("\\\\?\\")
    if not os.path.isabs(t):  # 别的程序建的符号链接可能是相对路径
        t = os.path.join(os.path.dirname(link), t)
    return os.path.normcase(os.path.abspath(t)) == os.path.normcase(os.path.abspath(target))


def remove_link(p):
    """只删目录链接本身，不碰它指向的文件夹。[基础设施]

    Windows 的 junction 用 rmdir，macOS 的符号链接用 unlink。

    Args:
        p: 链接路径
    """
    if os.name == "nt":
        os.rmdir(p)
    else:
        os.unlink(p)


def wait_for(cond, secs):
    """每秒看一次 cond，等它变成真。[基础设施]

    Args:
        cond: 不带参数的函数，返回值当布尔用
        secs: 最多等多少秒
    Returns:
        bool — 等到了是 True，到时间还没等到是 False
    """
    end = time.time() + secs
    while time.time() < end:
        if cond():
            return True
        time.sleep(1)
    return bool(cond())


def same_file(a, b):
    """两个路径是不是同一个文件；短文件名、大小写这些写法不同不算不同。[基础设施]

    Args:
        a: 路径；None、空字符串、文件不存在都算不是
        b: 路径，同上
    Returns:
        bool
    """
    try:
        return bool(a) and bool(b) and os.path.samefile(a, b)
    except OSError:
        return False


def registered_exe():
    """开机自启这种方式下，后台任务登记给了哪个程序：config.json 里 schedule.exe。[基础设施]

    Returns:
        str | None — 程序路径；没登记是 None
    """
    return (json.loads((HUB_DIR / "config.json").read_text(encoding="utf-8")).get("schedule") or {}).get("exe")


def daemon_exe():
    """在跑的后台进程是哪个程序：它自己写在 daemon.json 里的路径。[基础设施]

    Returns:
        str | None — 程序路径；后台进程没在跑（文件不在）或者是不写路径的旧版本，是 None
    """
    try:
        return json.loads((HUB_DIR / "daemon.json").read_text(encoding="utf-8")).get("exe")
    except (OSError, ValueError):
        return None


def startup_target():
    """Windows「启动」文件夹里那个快捷方式指向的程序。[基础设施]

    Returns:
        str — 程序路径；快捷方式不在或者读不出来是空字符串
    """
    script = ("[Console]::OutputEncoding = [Text.Encoding]::UTF8; "
              "$l = Join-Path ([Environment]::GetFolderPath('Startup')) '插件中心自动更新.lnk'; "
              "if (Test-Path -LiteralPath $l) { (New-Object -ComObject WScript.Shell).CreateShortcut($l).TargetPath }")
    r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script], capture_output=True, text=True, encoding="utf-8")
    return r.stdout.strip()


def open_program(exe):
    """像用户打开程序那样把 exe 开起来：浏览器版的页面服务，不弹浏览器。程序打开时会看一眼后台任务归不归自己。[基础设施]

    Args:
        exe: 程序路径
    Returns:
        subprocess.Popen — 页面服务的进程，用完交给 close_program；这个脚本中途没了它也会自己退出
    """
    return subprocess.Popen([str(exe), "--run", "serve", "--no-browser", "--port", "8790", "--owner", str(os.getpid())],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=ENV)


def close_program(proc):
    """关掉 open_program 开的页面服务，连它留下的 server.json 一起清掉。[基础设施]

    Args:
        proc: open_program 返回的进程
    """
    proc.terminate()
    proc.wait()
    info = HUB_DIR / "server.json"
    try:
        if json.loads(info.read_text(encoding="utf-8")).get("pid") == proc.pid:
            info.unlink()
    except (OSError, ValueError):
        pass


GUARD_BEFORE = bool(((api("state").get("hub") or {}).get("guard") or {}).get("enabled"))
# 在跑的后台进程要是 1.4.3 及以前的版本（daemon.json 里没写程序路径），它不认登记、不会让位，第 8b 步测试里开的后台进程接不上：
# 先把后台检查关掉，等它自己退出（收尾时本来也要关）
if GUARD_BEFORE and (HUB_DIR / "daemon.json").exists() and daemon_exe() is None:
    api("guard", {"enabled": False})
    wait_for(lambda: not (HUB_DIR / "daemon.json").exists(), 40)

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
url = "file:///" + str(bare).replace("\\", "/").lstrip("/")
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
remove_link(CLAUDE_SKILLS / ID)
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
check(f"--run guard 跑通且没问题时只报真实检查都通过（退出码 {code.returncode}）", code.returncode == 0 and ("都通过" in code.stdout or "都没问题" in code.stdout) and "已修复" not in code.stdout)

# 8. 后台任务的命令指向 exe 自己
st = api("state")
exe_l = str(EXE).lower()
check("后台任务交给程序自己", any(st["hub"]["launcher"].lower().startswith(x) for x in (f'"{exe_l}"', f"'{exe_l}'", exe_l)))
print("   launcher:", st["hub"]["launcher"])
notes = notes_of("budget")
check(f"单独看 Codex 的技能清单：{notes}", len(notes) == 1 and "[技能清单] Codex" in notes[0])

# 8b. 开机自启的后台进程（Windows 上建不了系统任务的机器）跟着程序走：登记的是哪个程序，快捷方式就指向它、在跑的就是它；
#     从别的位置打开程序，这三样都换过去。系统任务和 launchd 没有常驻的进程，不测这一段
if st["hub"]["auto"].get("mode") == "startup":
    check("登记的后台程序是它自己，快捷方式也指向它", same_file(registered_exe(), EXE) and same_file(startup_target(), EXE))
    # daemon.json 可能是上一次被强行结束的后台进程留下的，所以还要看后台进程是不是真的在
    ok = wait_for(lambda: same_file(daemon_exe(), EXE), 90) and api("state")["hub"]["auto"]["installed"]
    check("在跑的后台进程是它自己", ok)
    if not ok:
        print("   在跑的要是 1.4.3 及以前的后台进程，它不认登记、不会让位：先把后台检查关掉等半分钟，或者停掉那个进程，再测")
    other = work / "elsewhere" / EXE.name  # 同一个程序放到另一个位置，当成重装到了别处
    other.parent.mkdir()
    shutil.copy2(EXE, other)
    for exe, label in ((other, "从另一个位置打开程序"), (EXE, "再从原来的位置打开")):
        opened = open_program(exe)
        check(f"{label}：登记和快捷方式都改成指向它", wait_for(lambda: same_file(registered_exe(), exe), 30) and same_file(startup_target(), exe))
        check(f"{label}：原来的后台进程让位，它的接上", wait_for(lambda: same_file(daemon_exe(), exe), 90) and api("state")["hub"]["auto"]["installed"])
        close_program(opened)

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
    """state 里测试 skill 那一行，没有就是 None。[基础设施]

    Returns:
        dict | None
    """
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
remove_link(CLAUDE_SKILLS / SK)
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
# 12. 从 git 仓库装技能：把 npx skills add 命令粘进来。仓库里没有插件清单，只有 skills/ 下的两个技能
RID = f"{ID}-skills"
sk_src, sk_bare = work / "sk-src", work / f"{RID}.git"
for n in ("alpha", "beta"):
    (sk_src / "skills" / n).mkdir(parents=True)
    (sk_src / "skills" / n / "SKILL.md").write_text(f"---\nname: {ID}-{n}\ndescription: e2e {n} skill\n---\nv1\n")
git("init", "-b", "main", cwd=sk_src)
git("add", ".", cwd=sk_src)
git("commit", "-m", "v1", cwd=sk_src)
subprocess.run([g, "clone", "--bare", str(sk_src), str(sk_bare)], check=True, capture_output=True)
git("remote", "add", "origin", str(sk_bare), cwd=sk_src)
sk_url = "file:///" + str(sk_bare).replace("\\", "/").lstrip("/")
A, B = f"{ID}-alpha", f"{ID}-beta"
dup = HOME / ".agents" / "skills" / f"{B}-dup"
rmtree(dup)
probe = api("probe", {"repo": f"npx skills add {sk_url} --skill {A} -a claude-code,codex"})
main_b = next((b for b in probe.get("branches", []) if b["name"] == "main"), {})
check("npx 命令识别成技能，点到了 alpha", probe.get("mode") == "skills" and probe.get("input", {}).get("skills") == [A]
      and probe["input"]["agents"] == ["claude", "codex"] and {s["name"] for s in main_b.get("skills", [])} == {A, B}
      and probe.get("id") == RID and probe.get("managed") is False)
# 不经过页面：整条 npx 命令直接交给 add，分支和技能都由它自己认
res = api("add", {"repo": f"npx skills add {sk_url} --skill {A} -a claude-code,codex", "apps": ["claude", "codex"]})
print("   skills add:", "\n        ".join(res.get("notes") or [res.get("error")]))


def skrow(name):
    return next((r for r in api("state")["plugins"] if r["key"] == f"skill:{name}"), None)


def cfg_plugins():
    return [p["id"] for p in json.loads((HUB_DIR / "config.json").read_text(encoding="utf-8")).get("plugins", [])]


ours_a = PLUGINS_DIR / "skills" / "alpha"
r = skrow(A)
check("技能行带着仓库信息", r is not None and (r.get("managed") or {}).get("id") == RID and r["skill"]["ours"] and r["skill"].get("repo") == RID)
check("两边链接到统一存放处，统一存放处链接到克隆里的文件夹",
      is_link_to(CLAUDE_SKILLS / "alpha", ours_a) and is_link_to(CODEX_HOME / "skills" / "alpha", ours_a) and is_link_to(ours_a, PLUGINS_DIR / RID / "skills" / "alpha"))
check("装完的真实检查都通过", r is not None and all((a.get("verify") or {}).get("ok") for a in r["apps"]))
check("只拿技能的仓库不单独显示成插件卡片", not any(x["key"] == RID for x in api("state")["plugins"]))
(sk_src / "skills" / "alpha" / "SKILL.md").write_text(f"---\nname: {A}\ndescription: e2e alpha skill\n---\nv2\n")
git("commit", "-am", "v2", cwd=sk_src)
git("push", "origin", "main", cwd=sk_src)
api("check")
check("技能的仓库有新版本", ((skrow(A) or {}).get("managed") or {}).get("needs_update") is True)
res = api("sync", {"key": f"skill:{A}"})
check("同步拉到了新版本，两边读到的就是新文件", "v2" in (CLAUDE_SKILLS / "alpha" / "SKILL.md").read_text() and "error" not in res)
# Codex 还会读 ~/.agents/skills（npx skills 装技能的地方）：那里已经有同名的，就不往 Codex 再装一份
dup.mkdir(parents=True)
(dup / "SKILL.md").write_text(f"---\nname: {B}\ndescription: same name, installed by npx skills\n---\n")
main_b = next((b for b in api("probe", {"repo": sk_url}).get("branches", []) if b["name"] == "main"), {})
check("查询时标出 Codex 已经从 ~/.agents/skills 读到同名技能", next((s.get("local") for s in main_b.get("skills", []) if s["name"] == B), None) == ["agents"])
res = api("add", {"repo": sk_url, "branch": "main", "apps": ["claude", "codex"], "skills": ["skills/beta"]})
check("同名技能不往 Codex 重复装，也没留下半截的链接", "~/.agents/skills" in (res.get("error") or "") and not (CLAUDE_SKILLS / "beta").exists() and not (PLUGINS_DIR / "skills" / "beta").exists())
rmtree(dup)
res = api("add", {"repo": sk_url, "branch": "main", "apps": ["claude"], "skills": ["skills/beta"]})
check("同一个仓库再装一个技能", is_link_to(CLAUDE_SKILLS / "beta", PLUGINS_DIR / "skills" / "beta") and "error" not in res)

# 13. 只拿了技能的仓库后来有了插件清单：同一份克隆再装成插件；卸掉插件时技能还在用，克隆要留着
(sk_src / ".claude-plugin").mkdir()
(sk_src / ".codex-plugin").mkdir()
(sk_src / ".claude-plugin" / "plugin.json").write_text(json.dumps({"name": RID, "version": "0.0.1", "description": "e2e"}))
(sk_src / ".codex-plugin" / "plugin.json").write_text(json.dumps({"name": RID, "version": "0.0.1", "description": "e2e", "skills": "./skills/"}))
git("add", ".", cwd=sk_src)
git("commit", "-m", "v3 plugin", cwd=sk_src)
git("push", "origin", "main", cwd=sk_src)
probe = api("probe", {"repo": sk_url})
check("查询认出仓库已经在管理、现在也是插件了", probe.get("managed_as") == "skills" and probe.get("managed_branch") == "main" and probe.get("mode") == "plugin"
      and sorted(probe.get("managed_skills", [])) == ["skills/alpha", "skills/beta"])
res = api("add", {"repo": sk_url, "branch": "main", "apps": ["claude", "codex"]})
print("   plugin add:", "\n        ".join(res.get("notes") or [res.get("error")]))
pr = next((r for r in api("state")["plugins"] if r["key"] == RID), None)
check("同一份克隆装成了插件，两边都通过真实检查", pr is not None and {a["app"]: a.get("state") for a in pr["apps"]} == {"claude": "ok", "codex": "ok"}
      and all((a.get("verify") or {}).get("ok") for a in pr["apps"]))
check("原来的技能还在", is_link_to(CLAUDE_SKILLS / "alpha", ours_a) and (skrow(A) or {}).get("skill", {}).get("repo") == RID)
res = api("uninstall", {"key": RID})
print("   plugin uninstall:", "\n        ".join(res.get("notes") or [res.get("error")]))
check("卸掉插件：技能还在用，仓库和克隆都留着，只是不再装成插件",
      RID in cfg_plugins() and (PLUGINS_DIR / RID / ".git").is_dir() and not any(r["key"] == RID for r in api("state")["plugins"])
      and not (CLAUDE_SKILLS / RID).exists() and "v2" in (CLAUDE_SKILLS / "alpha" / "SKILL.md").read_text())

api("uninstall", {"key": f"skill:{A}"})
check("删掉 alpha 后仓库还在管理（beta 还在用）", RID in cfg_plugins() and not (CLAUDE_SKILLS / "alpha").exists() and not ours_a.is_symlink() and (PLUGINS_DIR / RID).is_dir())
api("uninstall", {"key": f"skill:{B}"})
check("技能都删了，仓库也不再管理，克隆挪进了备份", RID not in cfg_plugins() and not (PLUGINS_DIR / RID).exists() and any(BACKUP_DIR.glob(f"{RID}-*")))

check("Codex 的 config.toml 和测试前一样", (CODEX_HOME / "config.toml").read_text(encoding="utf-8") == CODEX_CONFIG_BEFORE)
check("Claude Code 的 settings.json 和测试前一样", json.loads(SETTINGS.read_text(encoding="utf-8")) == SETTINGS_BEFORE)

# 收尾：关掉测试里开的后台检查，顺带注销系统任务，别留下一个指向测试程序的 launchd 或计划任务；
# 测试前就是开着的话提醒一声（它原来指向的是装好的程序，这里不好原样恢复）
api("guard", {"enabled": False})
if GUARD_BEFORE:
    print("   提醒：测试前后台检查是开着的，测试结束时已关掉，请在插件中心里重新打开")

# 收尾：去掉测试插件和测试 skill 在 state.json 里的记录，备份和另存的文件夹，测试用的仓库
st_path = HUB_DIR / "state.json"
st = json.loads(st_path.read_text(encoding="utf-8"))
for k in ("verify", "repairs"):
    for key in [x for x in st.get(k, {}) if ID in x]:
        del st[k][key]
for key in [x for x in st.get("plugins", {}) if x.startswith(ID)]:
    del st["plugins"][key]
st_path.write_text(json.dumps(st, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
for d in [*BACKUP_DIR.glob(f"{ID}-*"), *BACKUP_DIR.glob(f"skill-{SK}-*"), *(SAVED_DIR.glob(f"{ID}-*") if SAVED_DIR.exists() else [])]:
    rmtree(d)
for d in (PLUGINS_DIR / "skills",):
    if d.exists() and not any(d.iterdir()):
        d.rmdir()
if SAVED_DIR.exists() and not any(SAVED_DIR.iterdir()):
    SAVED_DIR.rmdir()
# 第 8b 步拷到别处的那份程序要是还当着后台进程（换回来那一步没成），它占着自己的 exe，下面删不掉：
# 后台检查已经关了，等它看到以后自己退出
if same_file(daemon_exe(), work / "elsewhere" / EXE.name):
    wait_for(lambda: daemon_exe() is None, 40)
rmtree(work)
rmtree(dup)

print("RESULT:", "FAILED" if failed else "ALL PASSED")
sys.exit(1 if failed else 0)
