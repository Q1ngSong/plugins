"""发布一个新版本：python3 scripts/release.py 1.3.1

做的事：把四处版本号（和 Cargo.lock）改成新的，快速检查一遍代码能过，重新生成项目主页，
提交到 main，打 v1.3.1 的 tag，把 main 和 tag 一起推上 GitHub。之后 Actions 会构建两个平台的安装包，
建一个草稿 Release，到网页上检查、点 Publish 就发布了（装着旧版的插件中心会提示有新版本）。

先决条件：在 main 上、工作区干净、本地 main 和 origin/main 一致。不满足就停下，什么都不改。
"""
import json
import re
import subprocess
import sys
from pathlib import Path

HUB = Path(__file__).resolve().parent.parent  # pluginhub/
ROOT = HUB.parent                             # 仓库根目录


def run(*args, cwd=ROOT, capture=True):
    r = subprocess.run(args, cwd=cwd, text=True, capture_output=capture)
    if r.returncode != 0:
        sys.exit(f"命令失败：{' '.join(args)}\n{(r.stderr or r.stdout or '').strip()}")
    return (r.stdout or "").strip()


def replace(path, pattern, repl, count=1):
    text = path.read_text(encoding="utf-8")
    new, n = re.subn(pattern, repl, text, count=count, flags=re.M)
    if n != count:
        sys.exit(f"{path.relative_to(ROOT)} 里没找到要改的版本号")
    path.write_text(new, encoding="utf-8")


def main():
    if len(sys.argv) != 2 or not re.fullmatch(r"\d+\.\d+\.\d+", sys.argv[1]):
        sys.exit("用法：python3 scripts/release.py 1.3.1")
    version = sys.argv[1]
    tag = f"v{version}"
    current = json.loads((HUB / "package.json").read_text(encoding="utf-8"))["version"]
    if tuple(map(int, version.split("."))) <= tuple(map(int, current.split("."))):
        sys.exit(f"新版本 {version} 不比当前的 {current} 高")

    # 先把门槛检查做完，不满足就不动任何文件
    if run("git", "branch", "--show-current") != "main":
        sys.exit("要在 main 分支上发布")
    if run("git", "status", "--porcelain"):
        sys.exit("工作区有没提交的改动，先提交或收起来")
    run("git", "fetch", "origin", "main", "--tags")
    if run("git", "rev-parse", "main") != run("git", "rev-parse", "origin/main"):
        sys.exit("本地 main 和 origin/main 不一致，先同步")
    if run("git", "tag", "-l", tag):
        sys.exit(f"标签 {tag} 已经存在")

    # 快速检查：页面能构建，Rust 能编过（不打包，打包交给 Actions）。放在改文件之前，失败了工作区还是干净的。
    # 页面要先构建出 dist/，Rust 那边嵌页面时会找它
    print("检查页面和 Rust 代码……")
    run("pnpm", "build:renderer", cwd=HUB)
    run("cargo", "check", "--release", cwd=HUB / "src-tauri")

    # 四处版本号，加上 Cargo.lock 里自己这一项
    replace(HUB / "package.json", r'^(\s*"version": )"[^"]+"', rf'\g<1>"{version}"')
    replace(HUB / "src-tauri/tauri.conf.json", r'^(\s*"version": )"[^"]+"', rf'\g<1>"{version}"')
    replace(HUB / "src-tauri/Cargo.toml", r'^version = "[^"]+"', f'version = "{version}"')
    replace(HUB / "src-tauri/src/util.rs", r'^(pub const HUB_VERSION: &str = )"[^"]+"', rf'\g<1>"{version}"')
    replace(HUB / "src-tauri/Cargo.lock", r'^(name = "pluginhub"\nversion = )"[^"]+"', rf'\g<1>"{version}"')
    print(f"版本号 {current} → {version}")

    # 项目主页是从 README 生成的，顺手刷新，保证网站和仓库一致
    run(sys.executable, str(ROOT / "site/build.py"))

    run("git", "add", "-A")
    run("git", "commit", "-m", f"发布 {tag}")
    run("git", "tag", tag)
    run("git", "push", "origin", "main", tag)
    print(f"""已推送 {tag}。接下来：
  1. 看构建：https://github.com/Q1ngSong/plugins/actions
  2. 构建完成后检查草稿里的两个安装包，点 Publish：https://github.com/Q1ngSong/plugins/releases
  项目主页在 Pages 开着时会随 main 自动更新（Settings → Pages → main 分支根目录，只需设一次）。""")


if __name__ == "__main__":
    main()
