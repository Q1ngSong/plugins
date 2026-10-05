"""生成项目主页：把根目录的 README.md 渲染成 HTML，填进 site/index.html 模板，写到根目录的 index.html。

运行：python3 site/build.py（需要 markdown-it-py）
"""
import re
from pathlib import Path
from urllib.parse import urljoin

from markdown_it import MarkdownIt

ROOT = Path(__file__).resolve().parents[1]
README_URL = "https://github.com/Q1ngSong/plugins/blob/main/README.md"

# README 开头的标题、介绍和主图已经在模板的首屏里，从第一个二级标题开始渲染
text = (ROOT / "README.md").read_text(encoding="utf-8")
readme = MarkdownIt("commonmark").enable("table").render(text[text.index("\n## "):])
# 相对链接改成指向 GitHub（urljoin 不动完整的网址）；图片是 src，仍然用根目录的 assets/
readme = re.sub(r'href="([^"]+)"', lambda m: f'href="{urljoin(README_URL, m.group(1))}"', readme)
# 二级标题带上 id，模板里的 #安装 靠它跳转
readme = re.sub(r"<h2>([^<]+)</h2>", r'<h2 id="\1">\1</h2>', readme)

template = (ROOT / "site/index.html").read_text(encoding="utf-8")
(ROOT / "index.html").write_text(template.replace("__README__", readme), encoding="utf-8")
